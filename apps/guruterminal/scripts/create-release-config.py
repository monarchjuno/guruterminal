#!/usr/bin/env python3
"""Create the single Tauri config overlay used by a protected release build."""

from __future__ import annotations

import argparse
import json
import os
import re
from pathlib import Path
from urllib.parse import urlparse


STABLE_UPDATE_ENDPOINT = (
    "https://github.com/monarchjuno/guruterminal/releases/latest/download/latest.json"
)
MACOS_BUNDLE_VERSION = re.compile(r"^[1-9][0-9]*$")
RELEASE_VERSION = re.compile(
    r"^(?P<base>(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*))"
    r"(?:-rc\.(?P<rc>[1-9][0-9]*))?$"
)


def required_environment(name: str) -> str:
    value = os.environ.get(name, "").strip()
    if not value:
        raise RuntimeError(f"required environment variable is empty: {name}")
    return value


def https_url(value: str, label: str) -> str:
    parsed = urlparse(value)
    if (
        parsed.scheme != "https"
        or not parsed.netloc
        or parsed.username is not None
        or parsed.password is not None
        or parsed.fragment
    ):
        raise RuntimeError(
            f"{label} must be a credential-free HTTPS URL without a fragment"
        )
    return value


def update_endpoints(version: str) -> list[str]:
    match = RELEASE_VERSION.fullmatch(version)
    if match is None:
        raise RuntimeError(
            "--version must be canonical X.Y.Z or X.Y.Z-rc.N (N >= 1) without build metadata"
        )

    endpoints = [STABLE_UPDATE_ENDPOINT]
    rc_number = match.group("rc")
    if rc_number is not None:
        endpoints.insert(
            0,
            "https://github.com/monarchjuno/guruterminal/releases/download/"
            f"v{match.group('base')}-rc.{int(rc_number) + 1}/latest.json",
        )
    return endpoints


def macos_bundle_version(value: str | None) -> str:
    if value is None or MACOS_BUNDLE_VERSION.fullmatch(value) is None:
        raise RuntimeError(
            "--macos-bundle-version must be a positive decimal build counter"
        )
    return value


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", required=True, choices=("macos", "windows"))
    parser.add_argument("--version", required=True)
    parser.add_argument("--macos-bundle-version")
    parser.add_argument("--release-config", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()

    if os.environ.get("TAURI_CONFIG"):
        raise RuntimeError(
            "TAURI_CONFIG must be unset; the generated file is the only release overlay"
        )

    config = json.loads(arguments.release_config.read_text(encoding="utf-8"))
    updater = config.setdefault("plugins", {}).setdefault("updater", {})
    updater["pubkey"] = required_environment("GURUTERMINAL_UPDATER_PUBLIC_KEY")
    updater["endpoints"] = update_endpoints(arguments.version)

    if arguments.platform == "macos":
        macos = config.setdefault("bundle", {}).setdefault("macOS", {})
        macos["bundleVersion"] = macos_bundle_version(arguments.macos_bundle_version)
    elif arguments.macos_bundle_version is not None:
        raise RuntimeError("--macos-bundle-version is valid only for macos builds")
    else:
        thumbprint = required_environment("GURUTERMINAL_WINDOWS_CERTIFICATE_THUMBPRINT")
        if not re.fullmatch(r"[0-9A-Fa-f]{40}", thumbprint):
            raise RuntimeError("Windows certificate thumbprint must be 40 hex digits")
        windows = config.setdefault("bundle", {}).setdefault("windows", {})
        windows.update(
            {
                "certificateThumbprint": thumbprint,
                "digestAlgorithm": "sha256",
                "timestampUrl": https_url(
                    required_environment("GURUTERMINAL_WINDOWS_TIMESTAMP_URL"),
                    "GURUTERMINAL_WINDOWS_TIMESTAMP_URL",
                ),
            }
        )

    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    arguments.output.write_text(
        json.dumps(config, ensure_ascii=False, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )
    arguments.output.chmod(0o600)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
