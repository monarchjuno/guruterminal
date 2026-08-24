#!/usr/bin/env python3
"""Allocate a macOS build counter above every retained release artifact."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


POSITIVE_COUNTER = re.compile(r"^[1-9][0-9]*$")


def positive_counter(value: object, label: str) -> int:
    if not isinstance(value, str) or POSITIVE_COUNTER.fullmatch(value) is None:
        raise RuntimeError(f"{label} must be a positive decimal build counter")
    return int(value)


def retained_counters(metadata_directory: Path) -> list[int]:
    if not metadata_directory.is_dir() or metadata_directory.is_symlink():
        raise RuntimeError("release metadata directory must be a real directory")

    counters = []
    for metadata_path in sorted(metadata_directory.rglob("RELEASE-METADATA.json")):
        if metadata_path.is_symlink() or not metadata_path.is_file():
            raise RuntimeError(
                f"release metadata must be a regular file: {metadata_path}"
            )
        try:
            metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as error:
            raise RuntimeError(
                f"release metadata is invalid JSON: {metadata_path}"
            ) from error
        if not isinstance(metadata, dict) or metadata.get("schema_version") != 2:
            raise RuntimeError(
                f"release metadata must use schema version 2: {metadata_path}"
            )
        counters.append(
            positive_counter(
                metadata.get("macos_bundle_version"),
                f"release metadata macOS bundle version ({metadata_path})",
            )
        )
    return counters


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--run-number", required=True)
    parser.add_argument("--release-metadata-directory", required=True, type=Path)
    arguments = parser.parse_args()

    run_counter = positive_counter(arguments.run_number, "GitHub workflow run number")
    previous = max(retained_counters(arguments.release_metadata_directory), default=0)
    print(max(run_counter, previous + 1))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
