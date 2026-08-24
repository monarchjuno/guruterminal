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


def update_endpoint_url(value: str, label: str) -> str:
    value = https_url(value, label)
    parsed = urlparse(value)
    if parsed.hostname != "github.com" or parsed.port is not None or parsed.query:
        raise RuntimeError(f"{label} must use the canonical GitHub release host")
    stable_path = urlparse(STABLE_UPDATE_ENDPOINT).path
    exact_rc = re.fullmatch(
        r"/monarchjuno/guruterminal/releases/download/"
        r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)-rc\.([1-9]\d*)/latest\.json",
        parsed.path,
    )
    if parsed.path != stable_path and exact_rc is None:
        raise RuntimeError(f"{label} must be a Guru Terminal release manifest")
    return value


def update_endpoints() -> list[str]:
    name = "GURUTERMINAL_UPDATE_ENDPOINTS"
    try:
        values = json.loads(required_environment(name))
    except json.JSONDecodeError as error:
        raise RuntimeError(f"{name} must be a JSON array") from error
    if not isinstance(values, list) or not values:
        raise RuntimeError(f"{name} must be a nonempty JSON array")
    endpoints = []
    for index, value in enumerate(values):
        if not isinstance(value, str):
            raise RuntimeError(f"{name}[{index}] must be a string")
        endpoints.append(update_endpoint_url(value, f"{name}[{index}]"))
    if len(set(endpoints)) != len(endpoints):
        raise RuntimeError(f"{name} must not contain duplicate endpoints")
    if len(endpoints) > 2 or endpoints[-1] != STABLE_UPDATE_ENDPOINT:
        raise RuntimeError(
            f"{name} must end with the canonical stable endpoint and contain at most one RC endpoint"
        )
    if len(endpoints) == 2 and "-rc." not in endpoints[0]:
        raise RuntimeError(f"{name}[0] must be the exact next-RC endpoint")
    return endpoints


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", required=True, choices=("macos", "windows"))
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
    updater["endpoints"] = update_endpoints()

    if arguments.platform == "windows":
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
