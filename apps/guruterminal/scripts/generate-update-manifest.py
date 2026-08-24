#!/usr/bin/env python3
"""Build Tauri's static updater manifest from signed release artifacts."""

from __future__ import annotations

import argparse
import json
import re
from datetime import datetime
from pathlib import Path
from urllib.parse import quote

from release_asset_contract import updater_artifact_names, updater_signature_name


RELEASE_VERSION = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-rc\.([1-9]\d*))?$"
)


def signature_for(artifact: Path) -> str:
    signature = artifact.with_name(updater_signature_name(artifact.name))
    if not artifact.is_file() or not signature.is_file():
        raise RuntimeError(f"missing updater artifact or signature: {artifact}")
    value = signature.read_text(encoding="utf-8").strip()
    if not value:
        raise RuntimeError(f"updater signature is empty: {signature}")
    return value


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--pub-date", required=True)
    parser.add_argument("--assets", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()

    if not RELEASE_VERSION.fullmatch(arguments.version):
        raise RuntimeError("release version must be canonical X.Y.Z or X.Y.Z-rc.N")
    if arguments.tag != f"v{arguments.version}":
        raise RuntimeError("release tag must be v followed by the application version")
    datetime.fromisoformat(arguments.pub_date.replace("Z", "+00:00"))

    artifacts = {
        platform: arguments.assets / name
        for platform, name in updater_artifact_names(arguments.version).items()
    }
    base_url = (
        f"https://github.com/{arguments.repository}/releases/download/"
        f"{quote(arguments.tag, safe='')}"
    )
    platforms = {
        platform: {
            "url": f"{base_url}/{quote(artifact.name, safe='')}",
            "signature": signature_for(artifact),
        }
        for platform, artifact in artifacts.items()
    }
    manifest = {
        "version": arguments.version,
        "notes": f"Guru Terminal {arguments.version}",
        "pub_date": arguments.pub_date,
        "platforms": platforms,
    }
    arguments.output.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
