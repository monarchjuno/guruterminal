#!/usr/bin/env python3
"""Finalize one signed release candidate without rebuilding its platform assets."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
from pathlib import Path


RELEASE_VERSION = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-rc\.([1-9]\d*))?$"
)
REPOSITORY = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
SOURCE_COMMIT = re.compile(r"^[0-9a-f]{40}$")
MACOS_BUNDLE_VERSION = re.compile(r"^[1-9][0-9]*$")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def require_regular_file(path: Path) -> None:
    if path.is_symlink() or not path.is_file() or path.stat().st_size == 0:
        raise RuntimeError(f"release asset must be a nonempty regular file: {path}")


def canonical_names(version: str) -> list[str]:
    return [
        f"Guru Terminal-{version}-aarch64-apple-darwin.dmg",
        f"Guru Terminal-{version}-darwin-aarch64.app.tar.gz",
        f"Guru Terminal-{version}-darwin-aarch64.app.tar.gz.sig",
        f"Guru Terminal-{version}-x86_64-pc-windows-msvc-setup.exe",
        f"Guru Terminal-{version}-x86_64-pc-windows-msvc-setup.exe.sig",
        f"Guru Terminal-{version}.spdx.json",
        "latest.json",
    ]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--workflow-run-id", required=True)
    parser.add_argument("--macos-bundle-version", required=True)
    parser.add_argument("--assets", required=True, type=Path)
    arguments = parser.parse_args()

    if not RELEASE_VERSION.fullmatch(arguments.version):
        raise RuntimeError("release version must be canonical X.Y.Z or X.Y.Z-rc.N")
    if arguments.tag != f"v{arguments.version}":
        raise RuntimeError("release tag must be v followed by the application version")
    if not REPOSITORY.fullmatch(arguments.repository):
        raise RuntimeError("repository must be an owner/name pair")
    if not SOURCE_COMMIT.fullmatch(arguments.source_commit):
        raise RuntimeError("source commit must be a lowercase 40-digit SHA-1")
    if (
        not arguments.workflow_run_id.isascii()
        or not arguments.workflow_run_id.isdigit()
        or int(arguments.workflow_run_id) < 1
    ):
        raise RuntimeError("workflow run id must be a positive decimal integer")
    if MACOS_BUNDLE_VERSION.fullmatch(arguments.macos_bundle_version) is None:
        raise RuntimeError(
            "macOS bundle version must be a positive decimal build counter"
        )
    if not arguments.assets.is_dir() or arguments.assets.is_symlink():
        raise RuntimeError("release assets path must be a real directory")

    canonical = canonical_names(arguments.version)
    for name in canonical:
        require_regular_file(arguments.assets / name)

    aliases = {
        "GuruTerminal-macOS-arm64.dmg": canonical[0],
        "GuruTerminal-Windows-x64.exe": canonical[3],
    }
    reserved = [*aliases, "RELEASE-METADATA.json", "SHA256SUMS"]
    for name in reserved:
        path = arguments.assets / name
        if path.exists() or path.is_symlink():
            raise RuntimeError(
                f"refusing to replace an existing finalized asset: {path}"
            )

    for alias, source in aliases.items():
        shutil.copyfile(arguments.assets / source, arguments.assets / alias)

    metadata = {
        "schema_version": 2,
        "repository": arguments.repository,
        "tag": arguments.tag,
        "version": arguments.version,
        "source_commit": arguments.source_commit,
        "workflow_run_id": arguments.workflow_run_id,
        "macos_bundle_version": arguments.macos_bundle_version,
        "updater_manifest": "latest.json",
        "artifacts": {
            name: {"sha256": sha256(arguments.assets / name)} for name in canonical
        },
        "download_aliases": {
            alias: {
                "canonical": source,
                "sha256": sha256(arguments.assets / alias),
            }
            for alias, source in aliases.items()
        },
    }
    metadata_path = arguments.assets / "RELEASE-METADATA.json"
    metadata_path.write_text(
        json.dumps(metadata, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )

    checksum_names = sorted([*canonical, *aliases, metadata_path.name])
    checksum_path = arguments.assets / "SHA256SUMS"
    checksum_path.write_text(
        "".join(
            f"{sha256(arguments.assets / name)}  {name}\n" for name in checksum_names
        ),
        encoding="utf-8",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
