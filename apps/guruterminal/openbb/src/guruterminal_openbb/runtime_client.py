"""Small stdio client used by OpenBB bundle audits and opt-in live checks."""

from __future__ import annotations

import json
import os
import queue
import subprocess
import tempfile
import threading
import time
from collections import deque
from pathlib import Path
from typing import Any

from guruterminal_openbb.manifest import resolve_network_hosts

MCP_PROTOCOL_VERSION = "2025-06-18"
MAX_FRAME_BYTES = 32 * 1024 * 1024


class RuntimeClientError(RuntimeError):
    """A bounded, credential-free runtime client failure."""


def resolve_bundle(bundle: Path) -> tuple[Path, Path, dict[str, Any]]:
    """Resolve a staged onedir bundle and its public runtime manifest."""

    root = bundle.expanduser().resolve()
    if not root.is_dir():
        raise RuntimeClientError(f"OpenBB bundle directory does not exist: {root}")
    manifest_path = root / "runtime-manifest.json"
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise RuntimeClientError(
            f"OpenBB bundle manifest is unreadable: {manifest_path}"
        ) from error
    executable_name = manifest.get("executable")
    if not isinstance(executable_name, str) or not executable_name:
        raise RuntimeClientError("OpenBB bundle manifest has no executable")
    executable = root / executable_name
    if os.name == "nt" and executable.suffix.lower() != ".exe":
        executable = executable.with_suffix(".exe")
    if not executable.is_file():
        raise RuntimeClientError(f"OpenBB executable does not exist: {executable}")
    return executable, manifest_path, manifest


class RuntimeClient:
    """One isolated OpenBB MCP session with newline JSON-RPC framing."""

    def __init__(
        self,
        executable: Path,
        manifest: dict[str, Any],
        *,
        enabled_provider_ids: list[str],
        timeout: float = 45.0,
    ) -> None:
        self.executable = executable
        self.manifest = manifest
        self.enabled_provider_ids = enabled_provider_ids
        self.timeout = timeout
        self._process: subprocess.Popen[str] | None = None
        self._scratch: tempfile.TemporaryDirectory[str] | None = None
        self._stdout: queue.Queue[str | BaseException | None] = queue.Queue()
        self._stderr: deque[str] = deque(maxlen=40)
        self._request_id = 0

    def __enter__(self) -> RuntimeClient:
        self.start()
        return self

    def __exit__(self, *_args: object) -> None:
        self.close()

    def start(self) -> None:
        if self._process is not None:
            raise RuntimeClientError("OpenBB runtime client is already started")
        self._scratch = tempfile.TemporaryDirectory(prefix="guruterminal-openbb-live-")
        scratch = Path(self._scratch.name)
        if os.name != "nt":
            scratch.chmod(0o700)
        self._process = subprocess.Popen(
            [str(self.executable)],
            cwd=self.executable.parent,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            bufsize=1,
        )
        assert self._process.stdout is not None
        assert self._process.stderr is not None
        threading.Thread(
            target=self._read_stdout,
            args=(self._process.stdout,),
            daemon=True,
        ).start()
        threading.Thread(
            target=self._read_stderr,
            args=(self._process.stderr,),
            daemon=True,
        ).start()

        bootstrap = {
            "type": "guruterminal.bootstrap",
            "protocol_version": 1,
            "run_id": "openbb-live-parity",
            "scratch_dir": str(scratch),
            "credentials": {},
            "settings": {
                "allowed_categories": self.manifest["allowed_categories"],
                "enabled_provider_ids": self.enabled_provider_ids,
                "allowed_network_hosts": sorted(
                    resolve_network_hosts(set(self.enabled_provider_ids), self.manifest)
                ),
                "provider_config": {},
            },
        }
        self._write(bootstrap)
        initialized = self.request(
            "initialize",
            {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "Guru Terminal OpenBB live parity",
                    "version": "1",
                },
            },
        )
        if initialized.get("protocolVersion") != MCP_PROTOCOL_VERSION:
            raise RuntimeClientError("OpenBB returned an incompatible MCP version")
        self.notify("notifications/initialized", {})

    def _read_stdout(self, stream: Any) -> None:
        try:
            for line in stream:
                if len(line.encode("utf-8")) > MAX_FRAME_BYTES:
                    self._stdout.put(
                        RuntimeClientError("OpenBB MCP frame is too large")
                    )
                    return
                self._stdout.put(line)
        except BaseException as error:  # pragma: no cover - OS pipe failure
            self._stdout.put(error)
        finally:
            self._stdout.put(None)

    def _read_stderr(self, stream: Any) -> None:
        for line in stream:
            self._stderr.append(line.rstrip()[:1000])

    def _write(self, value: dict[str, Any]) -> None:
        process = self._process
        if process is None or process.stdin is None or process.poll() is not None:
            raise RuntimeClientError(self._exit_message())
        frame = json.dumps(value, separators=(",", ":"), ensure_ascii=False)
        if len(frame.encode("utf-8")) > MAX_FRAME_BYTES:
            raise RuntimeClientError("outgoing OpenBB MCP frame is too large")
        try:
            process.stdin.write(frame + "\n")
            process.stdin.flush()
        except BrokenPipeError as error:
            raise RuntimeClientError(self._exit_message()) from error

    def _exit_message(self) -> str:
        process = self._process
        code = process.poll() if process is not None else None
        detail = self._stderr[-1] if self._stderr else "no diagnostic"
        return f"OpenBB process exited unexpectedly (code={code}): {detail}"

    def request(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        self._request_id += 1
        request_id = self._request_id
        self._write(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "method": method,
                "params": params,
            }
        )
        deadline = time.monotonic() + self.timeout
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RuntimeClientError(f"OpenBB request timed out: {method}")
            try:
                frame = self._stdout.get(timeout=remaining)
            except queue.Empty as error:
                raise RuntimeClientError(
                    f"OpenBB request timed out: {method}"
                ) from error
            if frame is None:
                raise RuntimeClientError(self._exit_message())
            if isinstance(frame, BaseException):
                raise RuntimeClientError("failed to read OpenBB MCP output") from frame
            try:
                message = json.loads(frame)
            except json.JSONDecodeError as error:
                raise RuntimeClientError("OpenBB emitted invalid MCP JSON") from error
            if message.get("method") == "ping" and "id" in message:
                self._write({"jsonrpc": "2.0", "id": message["id"], "result": {}})
                continue
            if message.get("id") != request_id:
                continue
            if "error" in message:
                error_value = message["error"]
                if isinstance(error_value, dict):
                    detail = str(error_value.get("message", "MCP request failed"))
                else:
                    detail = "MCP request failed"
                raise RuntimeClientError(f"OpenBB {method} failed: {detail[:1000]}")
            result = message.get("result")
            if not isinstance(result, dict):
                raise RuntimeClientError(f"OpenBB {method} returned an invalid result")
            return result

    def notify(self, method: str, params: dict[str, Any]) -> None:
        self._write({"jsonrpc": "2.0", "method": method, "params": params})

    def call_tool(self, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        return self.request("tools/call", {"name": name, "arguments": arguments})

    def list_tools(self) -> list[dict[str, Any]]:
        tools: list[dict[str, Any]] = []
        cursor: str | None = None
        while True:
            params = {} if cursor is None else {"cursor": cursor}
            result = self.request("tools/list", params)
            page = result.get("tools")
            if not isinstance(page, list) or not all(
                isinstance(tool, dict) for tool in page
            ):
                raise RuntimeClientError("OpenBB returned an invalid Tool page")
            tools.extend(page)
            cursor_value = result.get("nextCursor")
            if cursor_value is None:
                return tools
            if not isinstance(cursor_value, str) or not cursor_value:
                raise RuntimeClientError("OpenBB returned an invalid Tool cursor")
            cursor = cursor_value

    def discover_all_tools(self) -> tuple[list[str], list[dict[str, Any]]]:
        category_result = self.call_tool("available_categories", {})
        structured = category_result.get("structuredContent", {}).get("result")
        if not isinstance(structured, list):
            raise RuntimeClientError("OpenBB category discovery returned invalid data")
        categories = sorted(
            item["name"]
            for item in structured
            if isinstance(item, dict) and isinstance(item.get("name"), str)
        )
        for category in categories:
            activation = self.call_tool("activate_category", {"category": category})
            if activation.get("isError") is True:
                raise RuntimeClientError(
                    f"OpenBB failed to activate category: {category}"
                )
        return categories, self.list_tools()

    def close(self) -> None:
        process = self._process
        self._process = None
        if process is not None:
            if process.stdin is not None:
                try:
                    process.stdin.close()
                except OSError:
                    pass
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.terminate()
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=3)
        if self._scratch is not None:
            self._scratch.cleanup()
            self._scratch = None
