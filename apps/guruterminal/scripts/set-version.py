#!/usr/bin/env python3
"""Validate or update the one product version across release manifests.

The release workflow deliberately rejects drift, but a version transition still
touches Rust, npm, Tauri, plist, and lockfile metadata. This command validates
the existing set before writing, then updates every authored copy together.
"""

from __future__ import annotations

import argparse
import json
import os
import plistlib
import re
import stat
import sys
import tempfile
import tomllib
from dataclasses import dataclass
from pathlib import Path


VERSION_PATTERN = re.compile(
    r"^(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-rc\.[1-9][0-9]*)?$"
)
ROOT_JSON_VERSION_PATTERN = re.compile(
    r'(?m)^(?P<prefix>  "version": ")(?P<version>[^"]+)(?P<suffix>"(?:,)?$)'
)
ROOT_PACKAGE_LOCK_VERSION_PATTERN = re.compile(
    r'(?ms)(?P<prefix>^    "": \{\n(?:(?!^    \}).)*?^      "version": ")'
    r'(?P<version>[^"]+)(?P<suffix>"(?:,)?$)'
)
CARGO_MANIFEST_VERSION_PATTERN = re.compile(
    r'(?m)^(?P<prefix>version = ")(?P<version>[^"]+)(?P<suffix>")$'
)
PLIST_SHORT_VERSION_PATTERN = re.compile(
    r"(?s)(?P<prefix><key>CFBundleShortVersionString</key>\s*<string>)"
    r"(?P<version>[^<]+)(?P<suffix></string>)"
)


class VersionError(ValueError):
    """Raised when the checked-in product identity is malformed or diverged."""


@dataclass(frozen=True)
class Document:
    path: Path
    contents: str


def canonical_version(value: str) -> str:
    if not VERSION_PATTERN.fullmatch(value):
        raise VersionError(
            "version must be canonical X.Y.Z or X.Y.Z-rc.N without build metadata"
        )
    return value


def base_version(value: str) -> str:
    return value.split("-", 1)[0]


def read_text(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except OSError as error:
        raise VersionError(f"{path}: {error}") from error


def parsed_json(path: Path) -> tuple[str, object]:
    contents = read_text(path)
    try:
        return contents, json.loads(contents)
    except json.JSONDecodeError as error:
        raise VersionError(f"{path}: invalid JSON: {error}") from error


def parsed_toml(path: Path) -> tuple[str, dict[str, object]]:
    contents = read_text(path)
    try:
        return contents, tomllib.loads(contents)
    except tomllib.TOMLDecodeError as error:
        raise VersionError(f"{path}: invalid TOML: {error}") from error


def replace_exactly_once(
    contents: str,
    pattern: re.Pattern[str],
    replacement: str,
    path: Path,
) -> str:
    matches = list(pattern.finditer(contents))
    if len(matches) != 1:
        raise VersionError(
            f"{path}: expected exactly one product version field, found {len(matches)}"
        )
    return pattern.sub(
        lambda match: f"{match.group('prefix')}{replacement}{match.group('suffix')}",
        contents,
        count=1,
    )


def cargo_manifest_version(path: Path) -> str:
    _, manifest = parsed_toml(path)
    package = manifest.get("package")
    if not isinstance(package, dict) or not isinstance(package.get("version"), str):
        raise VersionError(f"{path}: package.version is required")
    return canonical_version(package["version"])


def cargo_lock_version(path: Path, package_name: str) -> str:
    _, lock = parsed_toml(path)
    packages = lock.get("package")
    if not isinstance(packages, list):
        raise VersionError(f"{path}: package list is required")
    matching = [
        package
        for package in packages
        if isinstance(package, dict) and package.get("name") == package_name
    ]
    if len(matching) != 1 or not isinstance(matching[0].get("version"), str):
        raise VersionError(f"{path}: expected one {package_name} package version")
    return canonical_version(matching[0]["version"])


def cargo_lock_version_pattern(package_name: str) -> re.Pattern[str]:
    return re.compile(
        rf'(?ms)(?P<prefix>^\[\[package\]\]\nname = "{re.escape(package_name)}"\n'
        r'version = ")(?P<version>[^"]+)(?P<suffix>")'
    )


def package_json_version(path: Path) -> str:
    _, package = parsed_json(path)
    if not isinstance(package, dict) or not isinstance(package.get("version"), str):
        raise VersionError(f"{path}: version is required")
    return canonical_version(package["version"])


def package_lock_version(path: Path) -> str:
    _, lock = parsed_json(path)
    if not isinstance(lock, dict):
        raise VersionError(f"{path}: top-level object is required")
    version = lock.get("version")
    packages = lock.get("packages")
    root_package = packages.get("") if isinstance(packages, dict) else None
    root_version = (
        root_package.get("version") if isinstance(root_package, dict) else None
    )
    if not isinstance(version, str) or not isinstance(root_version, str):
        raise VersionError(f"{path}: root package version is required")
    if version != root_version:
        raise VersionError(f"{path}: lockfile root versions disagree")
    return canonical_version(version)


def plist_version(path: Path) -> str:
    try:
        with path.open("rb") as source:
            plist = plistlib.load(source)
    except (OSError, plistlib.InvalidFileException) as error:
        raise VersionError(f"{path}: invalid plist: {error}") from error
    value = plist.get("CFBundleShortVersionString") if isinstance(plist, dict) else None
    if not isinstance(value, str):
        raise VersionError(f"{path}: CFBundleShortVersionString is required")
    return canonical_version(value)


def current_version(root: Path) -> str:
    versions = {
        "core manifest": cargo_manifest_version(root / "Cargo.toml"),
        "core lockfile": cargo_lock_version(root / "Cargo.lock", "guruterminal-core"),
        "desktop package": package_json_version(
            root / "apps/guruterminal/package.json"
        ),
        "desktop package lock": package_lock_version(
            root / "apps/guruterminal/package-lock.json"
        ),
        "compute package": package_json_version(
            root / "apps/guruterminal/compute/package.json"
        ),
        "compute package lock": package_lock_version(
            root / "apps/guruterminal/compute/package-lock.json"
        ),
        "desktop manifest": cargo_manifest_version(
            root / "apps/guruterminal/src-tauri/Cargo.toml"
        ),
        "desktop lockfile": cargo_lock_version(
            root / "apps/guruterminal/src-tauri/Cargo.lock",
            "guruterminal-desktop",
        ),
        "Tauri configuration": package_json_version(
            root / "apps/guruterminal/src-tauri/tauri.conf.json"
        ),
    }
    distinct = set(versions.values())
    if len(distinct) != 1:
        details = ", ".join(f"{name}={value}" for name, value in versions.items())
        raise VersionError(f"product versions disagree: {details}")
    version = distinct.pop()
    plist = plist_version(root / "apps/guruterminal/src-tauri/Info.plist")
    if plist != base_version(version):
        raise VersionError(
            "Info.plist CFBundleShortVersionString must match the product base version"
        )
    return version


def rewritten_documents(root: Path, version: str) -> list[Document]:
    base = base_version(version)
    cargo_manifest_paths = [
        root / "Cargo.toml",
        root / "apps/guruterminal/src-tauri/Cargo.toml",
    ]
    cargo_lock_paths = [
        (root / "Cargo.lock", "guruterminal-core"),
        (
            root / "apps/guruterminal/src-tauri/Cargo.lock",
            "guruterminal-desktop",
        ),
    ]
    json_paths = [
        root / "apps/guruterminal/package.json",
        root / "apps/guruterminal/compute/package.json",
        root / "apps/guruterminal/src-tauri/tauri.conf.json",
    ]
    lock_paths = [
        root / "apps/guruterminal/package-lock.json",
        root / "apps/guruterminal/compute/package-lock.json",
    ]
    documents: list[Document] = []

    for path in cargo_manifest_paths:
        contents = read_text(path)
        documents.append(
            Document(
                path,
                replace_exactly_once(
                    contents, CARGO_MANIFEST_VERSION_PATTERN, version, path
                ),
            )
        )
    for path, package_name in cargo_lock_paths:
        contents = read_text(path)
        documents.append(
            Document(
                path,
                replace_exactly_once(
                    contents, cargo_lock_version_pattern(package_name), version, path
                ),
            )
        )
    for path in json_paths:
        contents = read_text(path)
        documents.append(
            Document(
                path,
                replace_exactly_once(
                    contents, ROOT_JSON_VERSION_PATTERN, version, path
                ),
            )
        )
    for path in lock_paths:
        contents = read_text(path)
        contents = replace_exactly_once(
            contents, ROOT_JSON_VERSION_PATTERN, version, path
        )
        documents.append(
            Document(
                path,
                replace_exactly_once(
                    contents, ROOT_PACKAGE_LOCK_VERSION_PATTERN, version, path
                ),
            )
        )

    plist_path = root / "apps/guruterminal/src-tauri/Info.plist"
    documents.append(
        Document(
            plist_path,
            replace_exactly_once(
                read_text(plist_path), PLIST_SHORT_VERSION_PATTERN, base, plist_path
            ),
        )
    )
    return documents


def atomic_write(path: Path, contents: str) -> None:
    mode = stat.S_IMODE(path.stat().st_mode)
    descriptor, temporary_path = tempfile.mkstemp(
        prefix=f".{path.name}.", dir=path.parent, text=True
    )
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="") as target:
            target.write(contents)
        os.chmod(temporary_path, mode)
        os.replace(temporary_path, path)
    except BaseException:
        try:
            os.unlink(temporary_path)
        except FileNotFoundError:
            pass
        raise


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Validate or update Guru Terminal's cross-manifest product version."
    )
    parser.add_argument("--version", help="canonical X.Y.Z or X.Y.Z-rc.N version")
    parser.add_argument(
        "--check",
        action="store_true",
        help="validate the existing product version without modifying files",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="show the files that would change without modifying them",
    )
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[3],
        help="repository root (defaults to this checkout)",
    )
    arguments = parser.parse_args()
    if arguments.check and arguments.dry_run:
        parser.error("--check and --dry-run cannot be used together")
    if not arguments.check and not arguments.version:
        parser.error("--version is required unless --check is used")
    if arguments.version:
        try:
            arguments.version = canonical_version(arguments.version)
        except VersionError as error:
            parser.error(str(error))
    return arguments


def main() -> int:
    arguments = parse_arguments()
    root = arguments.root.resolve()
    try:
        existing = current_version(root)
        if arguments.check:
            if arguments.version and arguments.version != existing:
                raise VersionError(
                    f"product version is {existing}, not requested {arguments.version}"
                )
            print(f"Product version is consistent: {existing}")
            return 0

        documents = rewritten_documents(root, arguments.version)
        changed = [
            document
            for document in documents
            if read_text(document.path) != document.contents
        ]
        if arguments.dry_run:
            for document in changed:
                print(document.path.relative_to(root))
            print(f"Would set product version from {existing} to {arguments.version}.")
            return 0

        for document in changed:
            atomic_write(document.path, document.contents)
        print(f"Set product version from {existing} to {arguments.version}.")
        return 0
    except VersionError as error:
        print(f"set-version: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
