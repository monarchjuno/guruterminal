#!/usr/bin/env python3
"""Normalize Tauri bundle output into immutable Guru Terminal release assets."""

from __future__ import annotations

import argparse
import shutil
from pathlib import Path

from release_asset_contract import (
    macos_installer_name,
    macos_updater_name,
    updater_signature_name,
    windows_updater_name,
)


def exactly_one(root: Path, pattern: str) -> Path:
    matches = sorted(path for path in root.rglob(pattern) if path.is_file())
    if len(matches) != 1:
        raise RuntimeError(
            f"expected exactly one {pattern!r} below {root}, found {len(matches)}"
        )
    return matches[0]


def copy(source: Path, output: Path, name: str) -> Path:
    destination = output / name
    shutil.copy2(source, destination)
    return destination


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", required=True, choices=("macos", "windows"))
    parser.add_argument("--version", required=True)
    parser.add_argument("--bundle-root", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()

    arguments.output.mkdir(parents=True, exist_ok=True)
    if arguments.platform == "macos":
        installer = exactly_one(arguments.bundle_root / "dmg", "*.dmg")
        updater = exactly_one(arguments.bundle_root / "macos", "*.app.tar.gz")
        signature = exactly_one(arguments.bundle_root / "macos", "*.app.tar.gz.sig")
        copy(
            installer,
            arguments.output,
            macos_installer_name(arguments.version),
        )
        updater_name = macos_updater_name(arguments.version)
        copy(updater, arguments.output, updater_name)
    else:
        updater = exactly_one(arguments.bundle_root / "nsis", "*-setup.exe")
        signature = exactly_one(arguments.bundle_root / "nsis", "*-setup.exe.sig")
        updater_name = windows_updater_name(arguments.version)
        copy(updater, arguments.output, updater_name)

    copy(signature, arguments.output, updater_signature_name(updater_name))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
