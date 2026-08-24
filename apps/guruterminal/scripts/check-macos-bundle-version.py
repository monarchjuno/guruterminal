#!/usr/bin/env python3
"""Validate Guru Terminal's source and packaged macOS bundle versions."""

from __future__ import annotations

import argparse
import json
import plistlib
import re
from pathlib import Path


SEMVER = re.compile(
    r"^(0|[1-9][0-9]*)\."
    r"(0|[1-9][0-9]*)\."
    r"(0|[1-9][0-9]*)"
    r"(?:-(?:[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
    r"(?:\+(?:[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$"
)
SHORT_VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")
BUNDLE_VERSION = re.compile(r"^[0-9]+(?:\.[0-9]+){0,2}$")
SHORT_VERSION_KEY = "CFBundleShortVersionString"
BUNDLE_VERSION_KEY = "CFBundleVersion"


def read_source(tauri_config: Path, source_plist: Path) -> tuple[str, str]:
    config = json.loads(tauri_config.read_text(encoding="utf-8"))
    try:
        source_version = config["version"]
        macos = config["bundle"]["macOS"]
        source_bundle_version = macos["bundleVersion"]
    except (KeyError, TypeError) as error:
        raise ValueError(f"missing Tauri macOS version authority: {error}") from error

    if not isinstance(source_version, str):
        raise ValueError("Tauri version must be a string")
    semver = SEMVER.fullmatch(source_version)
    if semver is None:
        raise ValueError("Tauri version must be valid SemVer")
    semver_base = ".".join(semver.groups()[:3])

    if not isinstance(source_bundle_version, str) or not BUNDLE_VERSION.fullmatch(
        source_bundle_version
    ):
        raise ValueError(
            "bundle.macOS.bundleVersion must contain 1-3 numeric components"
        )

    if "infoPlist" in macos:
        raise ValueError(
            "bundle.macOS.infoPlist must remain unset; Tauri auto-loads adjacent Info.plist"
        )

    with source_plist.open("rb") as source:
        plist = plistlib.load(source)
    if not isinstance(plist, dict) or set(plist) != {SHORT_VERSION_KEY}:
        raise ValueError(
            "adjacent Info.plist must override only CFBundleShortVersionString"
        )
    short_version = plist.get(SHORT_VERSION_KEY)
    if not isinstance(short_version, str) or not SHORT_VERSION.fullmatch(short_version):
        raise ValueError(
            "CFBundleShortVersionString must contain exactly 3 numeric components"
        )
    if short_version != semver_base:
        raise ValueError(
            "Tauri SemVer base must equal Info.plist CFBundleShortVersionString"
        )

    return short_version, source_bundle_version


def check_packaged_plist(
    bundle_plist: Path, source_short_version: str, source_bundle_version: str
) -> None:
    with bundle_plist.open("rb") as source:
        plist = plistlib.load(source)
    packaged_short_version = plist.get(SHORT_VERSION_KEY)
    packaged_bundle_version = plist.get(BUNDLE_VERSION_KEY)
    if packaged_short_version != source_short_version:
        raise ValueError(
            "packaged CFBundleShortVersionString differs from the adjacent source Info.plist"
        )
    if packaged_bundle_version != source_bundle_version:
        raise ValueError(
            "packaged CFBundleVersion differs from bundle.macOS.bundleVersion"
        )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tauri-config", required=True, type=Path)
    parser.add_argument("--source-plist", required=True, type=Path)
    parser.add_argument("--bundle-plist", type=Path)
    arguments = parser.parse_args()

    short_version, bundle_version = read_source(
        arguments.tauri_config, arguments.source_plist
    )
    if arguments.bundle_plist is not None:
        check_packaged_plist(arguments.bundle_plist, short_version, bundle_version)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
