#!/usr/bin/env python3
"""Fail closed unless a candidate is complete and internally byte-consistent."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from datetime import datetime
from pathlib import Path
from urllib.parse import quote


RELEASE_VERSION = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-rc\.([1-9]\d*))?$"
)
REPOSITORY = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
SOURCE_COMMIT = re.compile(r"^[0-9a-f]{40}$")
CHECKSUM_LINE = re.compile(r"^([0-9a-f]{64})  ([^/\\]+)$")
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


def parse_checksums(path: Path) -> dict[str, str]:
    checksums: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        match = CHECKSUM_LINE.fullmatch(line)
        if match is None:
            raise RuntimeError(f"malformed SHA256SUMS line: {line!r}")
        digest, name = match.groups()
        if name in {".", "..", "SHA256SUMS"} or name in checksums:
            raise RuntimeError(f"invalid or duplicate SHA256SUMS asset: {name}")
        checksums[name] = digest
    return checksums


def read_signature(path: Path) -> str:
    value = path.read_text(encoding="utf-8").strip()
    if not value:
        raise RuntimeError(f"updater signature is empty: {path}")
    return value


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--version", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--source-commit")
    parser.add_argument("--assets", required=True, type=Path)
    arguments = parser.parse_args()

    if not RELEASE_VERSION.fullmatch(arguments.version):
        raise RuntimeError("release version must be canonical X.Y.Z or X.Y.Z-rc.N")
    if arguments.tag != f"v{arguments.version}":
        raise RuntimeError("release tag must be v followed by the application version")
    if not REPOSITORY.fullmatch(arguments.repository):
        raise RuntimeError("repository must be an owner/name pair")
    if arguments.source_commit is not None and not SOURCE_COMMIT.fullmatch(
        arguments.source_commit
    ):
        raise RuntimeError("source commit must be a lowercase 40-digit SHA-1")
    if not arguments.assets.is_dir() or arguments.assets.is_symlink():
        raise RuntimeError("release assets path must be a real directory")

    canonical = canonical_names(arguments.version)
    aliases = {
        "GuruTerminal-macOS-arm64.dmg": canonical[0],
        "GuruTerminal-Windows-x64.exe": canonical[3],
    }
    expected = {*canonical, *aliases, "RELEASE-METADATA.json", "SHA256SUMS"}
    actual = {path.name for path in arguments.assets.iterdir()}
    if actual != expected:
        raise RuntimeError(
            "release asset set differs from the closed manifest: "
            f"missing={sorted(expected - actual)}, unexpected={sorted(actual - expected)}"
        )
    for name in expected:
        require_regular_file(arguments.assets / name)

    checksums = parse_checksums(arguments.assets / "SHA256SUMS")
    checksummed = expected - {"SHA256SUMS"}
    if set(checksums) != checksummed:
        raise RuntimeError("SHA256SUMS must cover every release asset exactly once")
    for name, expected_digest in checksums.items():
        actual_digest = sha256(arguments.assets / name)
        if actual_digest != expected_digest:
            raise RuntimeError(f"SHA-256 mismatch for {name}")

    metadata = json.loads(
        (arguments.assets / "RELEASE-METADATA.json").read_text(encoding="utf-8")
    )
    required_metadata = {
        "schema_version",
        "repository",
        "tag",
        "version",
        "source_commit",
        "workflow_run_id",
        "macos_bundle_version",
        "updater_manifest",
        "artifacts",
        "download_aliases",
    }
    if set(metadata) != required_metadata or metadata["schema_version"] != 2:
        raise RuntimeError("release metadata schema is not exactly version 2")
    if (
        metadata["repository"] != arguments.repository
        or metadata["tag"] != arguments.tag
        or metadata["version"] != arguments.version
        or metadata["updater_manifest"] != "latest.json"
    ):
        raise RuntimeError("release metadata identity does not match the candidate")
    if not SOURCE_COMMIT.fullmatch(metadata["source_commit"]):
        raise RuntimeError("release metadata source commit is invalid")
    if (
        arguments.source_commit is not None
        and metadata["source_commit"] != arguments.source_commit
    ):
        raise RuntimeError(
            "release metadata source commit does not match the requested commit"
        )
    workflow_run_id = metadata["workflow_run_id"]
    if (
        not isinstance(workflow_run_id, str)
        or not workflow_run_id.isascii()
        or not workflow_run_id.isdigit()
        or int(workflow_run_id) < 1
    ):
        raise RuntimeError("release metadata workflow run id is invalid")
    macos_bundle_version = metadata["macos_bundle_version"]
    if (
        not isinstance(macos_bundle_version, str)
        or MACOS_BUNDLE_VERSION.fullmatch(macos_bundle_version) is None
    ):
        raise RuntimeError("release metadata macOS bundle version is invalid")

    expected_artifacts = {
        name: {"sha256": sha256(arguments.assets / name)} for name in canonical
    }
    if metadata["artifacts"] != expected_artifacts:
        raise RuntimeError("release metadata artifact digests do not match")
    expected_aliases = {
        alias: {"canonical": source, "sha256": sha256(arguments.assets / alias)}
        for alias, source in aliases.items()
    }
    if metadata["download_aliases"] != expected_aliases:
        raise RuntimeError("release metadata aliases do not match")
    for alias, source in aliases.items():
        if sha256(arguments.assets / alias) != sha256(arguments.assets / source):
            raise RuntimeError(f"download alias does not preserve bytes: {alias}")

    manifest = json.loads(
        (arguments.assets / "latest.json").read_text(encoding="utf-8")
    )
    if set(manifest) != {"version", "notes", "pub_date", "platforms"}:
        raise RuntimeError("updater manifest has unexpected top-level fields")
    if manifest["version"] != arguments.version or not isinstance(
        manifest["notes"], str
    ):
        raise RuntimeError("updater manifest identity is invalid")
    published_at = datetime.fromisoformat(manifest["pub_date"].replace("Z", "+00:00"))
    if published_at.tzinfo is None:
        raise RuntimeError("updater manifest publication time must include a timezone")
    updater_artifacts = {
        "darwin-aarch64": canonical[1],
        "windows-x86_64": canonical[3],
    }
    base_url = (
        f"https://github.com/{arguments.repository}/releases/download/"
        f"{quote(arguments.tag, safe='')}"
    )
    expected_platforms = {
        platform: {
            "url": f"{base_url}/{quote(name, safe='')}",
            "signature": read_signature(arguments.assets / f"{name}.sig"),
        }
        for platform, name in updater_artifacts.items()
    }
    if manifest["platforms"] != expected_platforms:
        raise RuntimeError(
            "updater manifest URLs or signatures do not match the assets"
        )

    json.loads(
        (arguments.assets / f"Guru Terminal-{arguments.version}.spdx.json").read_text(
            encoding="utf-8"
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
