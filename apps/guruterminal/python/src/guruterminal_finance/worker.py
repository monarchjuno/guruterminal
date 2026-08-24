"""Persistent NDJSON / JSON-RPC 2.0 worker over stdin and stdout."""

from __future__ import annotations

import json
import platform
import sys
import threading
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from hashlib import sha256
from pathlib import Path
from typing import Any, TextIO

from .errors import WorkerError
from .schemas import PROTOCOL_VERSION, WORKER_VERSION
from .tools import execute_tool, list_tools

MAX_FRAME_BYTES = 4 * 1024 * 1024
MAX_WORKERS = 4


@dataclass(slots=True)
class ActiveRequest:
    cancel_event: threading.Event


class WorkerServer:
    def __init__(self, stdin: TextIO, stdout: TextIO) -> None:
        self.stdin = stdin
        self.stdout = stdout
        self._write_lock = threading.Lock()
        self._active_lock = threading.Lock()
        self._active: dict[str | int, ActiveRequest] = {}
        self._handshake_complete = False
        self._stopping = False
        self._executor = ThreadPoolExecutor(
            max_workers=MAX_WORKERS, thread_name_prefix="guru-finance"
        )

    def serve(self) -> int:
        try:
            for raw_line in self.stdin:
                if self._stopping:
                    break
                if not raw_line.strip():
                    continue
                self._handle_line(raw_line)
                if self._stopping:
                    break
        finally:
            if self._stopping:
                with self._active_lock:
                    for active in self._active.values():
                        active.cancel_event.set()
            self._executor.shutdown(wait=True, cancel_futures=False)
        return 0

    def _handle_line(self, raw_line: str) -> None:
        encoded_line = raw_line.encode("utf-8")
        frame_size = len(encoded_line)
        if encoded_line.endswith(b"\n"):
            frame_size -= 1
        if encoded_line[:frame_size].endswith(b"\r"):
            frame_size -= 1
        if frame_size > MAX_FRAME_BYTES:
            self._write_error(None, -32600, "Request exceeds the maximum line size")
            return
        try:
            message = json.loads(
                raw_line,
                parse_constant=lambda value: (_ for _ in ()).throw(
                    ValueError(f"Non-finite JSON number: {value}")
                ),
            )
        except (json.JSONDecodeError, ValueError):
            self._write_error(None, -32700, "Parse error")
            return
        if not isinstance(message, dict):
            self._write_error(None, -32600, "Invalid Request")
            return
        request_id = message.get("id")
        has_id = "id" in message
        if has_id and (
            isinstance(request_id, bool)
            or request_id is None
            or not isinstance(request_id, (str, int))
        ):
            self._write_error(None, -32600, "Request id must be a string or integer")
            return
        if message.get("jsonrpc") != "2.0" or not isinstance(
            message.get("method"), str
        ):
            if has_id:
                self._write_error(request_id, -32600, "Invalid Request")
            return
        params = message.get("params", {})
        if not isinstance(params, dict):
            if has_id:
                self._write_error(request_id, -32602, "params must be an object")
            return
        method = message["method"]
        if not has_id:
            self._handle_notification(method, params)
            return
        if method == "system.handshake":
            self._handle_handshake(request_id, params)
        elif method == "system.shutdown":
            self._handle_shutdown(request_id)
        elif not self._handshake_complete:
            self._write_error(
                request_id,
                -32002,
                "system.handshake must complete before this method",
                {"kind": "handshake_required"},
            )
        elif method == "tools.list":
            self._write_result(request_id, {"tools": list_tools()})
        elif method == "tools.call":
            self._schedule_tool_call(request_id, params)
        elif method == "system.cancel":
            self._handle_cancel(request_id, params)
        else:
            self._write_error(request_id, -32601, "Method not found")

    def _handle_notification(self, method: str, params: dict[str, Any]) -> None:
        if method != "system.cancel":
            return
        self._cancel(params.get("request_id"))

    def _cancel(self, request_id: object) -> bool:
        if (
            isinstance(request_id, bool)
            or request_id is None
            or not isinstance(request_id, (str, int))
        ):
            return False
        with self._active_lock:
            active = self._active.get(request_id)
        if active is not None:
            active.cancel_event.set()
            return True
        return False

    def _handle_cancel(self, request_id: str | int, params: dict[str, Any]) -> None:
        if set(params) != {"request_id"}:
            self._write_error(
                request_id,
                -32602,
                "system.cancel requires only request_id",
            )
            return
        target = params["request_id"]
        if (
            isinstance(target, bool)
            or target is None
            or not isinstance(target, (str, int))
        ):
            self._write_error(
                request_id,
                -32602,
                "system.cancel request_id must be a string or integer",
            )
            return
        self._write_result(
            request_id,
            {"request_id": target, "accepted": self._cancel(target)},
        )

    def _handle_handshake(self, request_id: str | int, params: dict[str, Any]) -> None:
        if not set(params) <= {"protocol_version", "client"}:
            self._write_error(
                request_id,
                -32602,
                "Handshake accepts only protocol_version and client",
            )
            return
        if params.get("protocol_version", PROTOCOL_VERSION) != PROTOCOL_VERSION:
            self._write_error(
                request_id,
                -32003,
                "Unsupported protocol version",
                {
                    "kind": "protocol_mismatch",
                    "supported": [PROTOCOL_VERSION],
                },
            )
            return
        client = params.get("client")
        if client is not None:
            if not isinstance(client, dict) or set(client) != {"name", "version"}:
                self._write_error(
                    request_id,
                    -32602,
                    "client requires only non-empty name and version strings",
                )
                return
            if any(
                not isinstance(client.get(key), str)
                or not client[key].strip()
                or len(client[key]) > 128
                for key in ("name", "version")
            ):
                self._write_error(
                    request_id,
                    -32602,
                    "client requires only non-empty name and version strings",
                )
                return
        self._handshake_complete = True
        self._write_result(
            request_id,
            {
                "protocol_version": PROTOCOL_VERSION,
                "worker_version": WORKER_VERSION,
                "python_version": platform.python_version(),
                "lock_digest": _lock_digest(),
                "tools": [tool["name"] for tool in list_tools()],
                "transport": "ndjson-stdio",
                "capabilities": {
                    "progress": True,
                    "cancellation": True,
                    "timeouts": True,
                    "read_only": True,
                    "arbitrary_code": False,
                    "http_server": False,
                },
            },
        )

    def _handle_shutdown(self, request_id: str | int) -> None:
        self._write_result(request_id, {"stopping": True})
        self._stopping = True

    def _schedule_tool_call(
        self, request_id: str | int, params: dict[str, Any]
    ) -> None:
        if set(params) != {"name", "arguments", "context"}:
            self._write_error(
                request_id,
                -32602,
                "tools.call requires only name, arguments, and context",
            )
            return
        active = ActiveRequest(cancel_event=threading.Event())
        with self._active_lock:
            if request_id in self._active:
                self._write_error(request_id, -32600, "Duplicate active request id")
                return
            self._active[request_id] = active

        def progress(stage: str, completed: int, total: int) -> None:
            self._write(
                {
                    "jsonrpc": "2.0",
                    "method": "progress",
                    "params": {
                        "id": request_id,
                        "stage": stage,
                        "completed": completed,
                        "total": total,
                    },
                }
            )

        def run_call() -> None:
            try:
                result = execute_tool(
                    params["name"],
                    params["arguments"],
                    params["context"],
                    cancel_event=active.cancel_event,
                    progress_callback=progress,
                )
                self._write_result(request_id, result)
            except WorkerError as error:
                self._write_error(request_id, error.code, error.message, error.data)
            except Exception:
                self._write_error(
                    request_id,
                    -32603,
                    "Internal error",
                    {"kind": "internal_error"},
                )
            finally:
                with self._active_lock:
                    self._active.pop(request_id, None)

        self._executor.submit(run_call)

    def _write_result(self, request_id: str | int, result: object) -> None:
        self._write({"jsonrpc": "2.0", "id": request_id, "result": result})

    def _write_error(
        self,
        request_id: str | int | None,
        code: int,
        message: str,
        data: dict[str, Any] | None = None,
    ) -> None:
        error: dict[str, Any] = {"code": code, "message": message}
        if data is not None:
            error["data"] = data
        self._write({"jsonrpc": "2.0", "id": request_id, "error": error})

    def _write(self, message: object) -> None:
        encoded = json.dumps(
            message,
            ensure_ascii=False,
            allow_nan=False,
            separators=(",", ":"),
            sort_keys=True,
        )
        with self._write_lock:
            self.stdout.write(encoded + "\n")
            self.stdout.flush()


def _lock_digest() -> str:
    candidates = []
    frozen_root = getattr(sys, "_MEIPASS", None)
    if frozen_root is not None:
        candidates.append(Path(frozen_root) / "uv.lock")
    candidates.append(Path(__file__).resolve().parents[2] / "uv.lock")
    for candidate in candidates:
        if candidate.is_file():
            return sha256(candidate.read_bytes()).hexdigest()
    return "unavailable"


def main() -> int:
    return WorkerServer(sys.stdin, sys.stdout).serve()


if __name__ == "__main__":
    raise SystemExit(main())
