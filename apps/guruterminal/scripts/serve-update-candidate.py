#!/usr/bin/env python3
"""Serve a local draft as github.com inside an isolated updater test machine."""

from __future__ import annotations

import argparse
import json
import mimetypes
import re
import ssl
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import unquote, urlsplit

from release_asset_contract import (
    METADATA_REPOSITORY,
    METADATA_SCHEMA_VERSION,
    METADATA_TAG,
    METADATA_VERSION,
    RELEASE_METADATA_NAME,
    RELEASE_METADATA_SCHEMA,
    UPDATER_MANIFEST_NAME,
    updater_artifact_names,
)


STABLE_VERSION = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")


def require_file(path: Path) -> Path:
    resolved = path.resolve(strict=True)
    if path.is_symlink() or not resolved.is_file() or resolved.stat().st_size == 0:
        raise RuntimeError(f"candidate asset must be a nonempty regular file: {path}")
    return resolved


def routes(assets: Path, repository: str) -> dict[str, Path]:
    if assets.is_symlink() or not assets.is_dir():
        raise RuntimeError("candidate assets path must be a real directory")
    if re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository) is None:
        raise RuntimeError("repository must be an owner/name pair")
    metadata_path = require_file(assets / RELEASE_METADATA_NAME)
    metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
    if metadata.get(METADATA_SCHEMA_VERSION) != RELEASE_METADATA_SCHEMA:
        raise RuntimeError("candidate release metadata schema is unsupported")
    if metadata.get(METADATA_REPOSITORY) != repository:
        raise RuntimeError("candidate repository does not match the proxy repository")
    tag = metadata.get(METADATA_TAG)
    version = metadata.get(METADATA_VERSION)
    if (
        not isinstance(tag, str)
        or not isinstance(version, str)
        or STABLE_VERSION.fullmatch(version) is None
        or tag != f"v{version}"
    ):
        raise RuntimeError("candidate metadata tag and version are invalid")
    manifest = require_file(assets / UPDATER_MANIFEST_NAME)
    updater_names = updater_artifact_names(version).values()
    prefix = f"/{repository}/releases"
    result = {f"{prefix}/latest/download/{UPDATER_MANIFEST_NAME}": manifest}
    for name in updater_names:
        result[f"{prefix}/download/{tag}/{name}"] = require_file(assets / name)
    return result


def handler_for(route_map: dict[str, Path]) -> type[BaseHTTPRequestHandler]:
    class CandidateHandler(BaseHTTPRequestHandler):
        server_version = "GuruTerminalQualification/1"
        sys_version = ""

        def do_HEAD(self) -> None:  # noqa: N802
            self._serve(send_body=False)

        def do_GET(self) -> None:  # noqa: N802
            self._serve(send_body=True)

        def _serve(self, *, send_body: bool) -> None:
            if self.headers.get("Host", "").partition(":")[0].lower() != "github.com":
                self.send_error(421)
                return
            parsed = urlsplit(self.path)
            if parsed.query:
                self.send_error(400)
                return
            path = route_map.get(unquote(parsed.path))
            if path is None:
                self.send_error(404)
                return
            content_type = (
                mimetypes.guess_type(path.name)[0] or "application/octet-stream"
            )
            self.send_response(200)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(path.stat().st_size))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.end_headers()
            if send_body:
                with path.open("rb") as source:
                    while chunk := source.read(1024 * 1024):
                        self.wfile.write(chunk)

        def log_message(self, format: str, *args: object) -> None:
            return

    return CandidateHandler


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--assets", required=True, type=Path)
    parser.add_argument("--repository", default="monarchjuno/guruterminal")
    parser.add_argument("--certificate", required=True, type=Path)
    parser.add_argument("--private-key", required=True, type=Path)
    parser.add_argument("--port", default=443, type=int)
    arguments = parser.parse_args()
    if arguments.port < 1 or arguments.port > 65535:
        raise RuntimeError("port must be between 1 and 65535")
    route_map = routes(arguments.assets, arguments.repository)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(
        require_file(arguments.certificate), require_file(arguments.private_key)
    )
    server = ThreadingHTTPServer(("127.0.0.1", arguments.port), handler_for(route_map))
    server.socket = context.wrap_socket(server.socket, server_side=True)
    print(
        f"candidate feed ready on https://github.com:{arguments.port}; "
        "press Ctrl-C to stop",
        flush=True,
    )
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
