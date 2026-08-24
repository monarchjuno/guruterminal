"""Build a deterministic legal-metadata archive for the frozen OpenBB runtime."""

from __future__ import annotations

import hashlib
import importlib.metadata
import json
import platform
import re
import shutil
import sys
import tomllib
from pathlib import Path, PurePosixPath
from typing import Any


ARCHIVE_DIRECTORY = "THIRD_PARTY_LICENSES"
MANIFEST_NAME = "python-distributions.json"
SCHEMA_VERSION = "guruterminal-python-licenses/1"
ROOT_DISTRIBUTION = "guruterminal-openbb"
_LEGAL_NAME = re.compile(
    r"(?:^|[-_.])(license|licence|copying|notice|copyright|authors?)"
    r"(?:[-_.]|$)",
    re.IGNORECASE,
)
_NON_LEGAL_SUFFIXES = {
    ".dll",
    ".dylib",
    ".exe",
    ".json",
    ".py",
    ".pyc",
    ".pyd",
    ".so",
}
_PACKAGE_OVERRIDES = {
    "pyluach": (
        "pyluach-2.3.0.txt",
        "https://github.com/simlist/pyluach/blob/v2.3.0/license.txt",
    ),
    "random-user-agent": (
        "random-user-agent-1.0.1.txt",
        "https://github.com/Luqman-Ud-Din/random_user_agent/blob/master/LICENSE",
    ),
}


class LicenseBundleError(RuntimeError):
    """The compliance archive could not be created or did not validate."""


def canonicalize_name(name: str) -> str:
    """Return the normalized distribution name used by uv and PyPA metadata."""

    return re.sub(r"[-_.]+", "-", name).lower()


def _file_digest(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def _lock_packages(lock_path: Path) -> dict[str, dict[str, Any]]:
    lock = tomllib.loads(lock_path.read_text(encoding="utf-8"))
    packages: dict[str, dict[str, Any]] = {}
    for package in lock.get("package", []):
        name = canonicalize_name(str(package.get("name", "")))
        if not name:
            raise LicenseBundleError("uv.lock contains a package without a name")
        if name in packages:
            raise LicenseBundleError(f"uv.lock contains duplicate package {name}")
        packages[name] = package
    if ROOT_DISTRIBUTION not in packages:
        raise LicenseBundleError("uv.lock is missing the OpenBB root distribution")
    return packages


def _dependency_extras(dependency: dict[str, Any]) -> tuple[str, ...]:
    value = dependency.get("extra", dependency.get("extras", ()))
    if isinstance(value, str):
        return (value,)
    if isinstance(value, list):
        return tuple(str(item) for item in value)
    return ()


def locked_runtime_closure(lock_path: Path) -> dict[str, dict[str, Any]]:
    """Resolve runtime dependencies from uv.lock, propagating requested extras."""

    packages = _lock_packages(lock_path)
    pending: list[tuple[str, tuple[str, ...]]] = [(ROOT_DISTRIBUTION, ())]
    regular_processed: set[str] = set()
    processed_extras: dict[str, set[str]] = {}
    selected: set[str] = set()

    while pending:
        raw_name, extras = pending.pop()
        name = canonicalize_name(raw_name)
        package = packages.get(name)
        if package is None:
            raise LicenseBundleError(f"uv.lock dependency is missing: {name}")
        selected.add(name)

        if name not in regular_processed:
            regular_processed.add(name)
            for dependency in package.get("dependencies", []):
                pending.append(
                    (
                        str(dependency["name"]),
                        _dependency_extras(dependency),
                    )
                )

        requested = {str(extra) for extra in extras}
        new_extras = requested - processed_extras.setdefault(name, set())
        processed_extras[name].update(new_extras)
        optional = package.get("optional-dependencies", {})
        for extra in sorted(new_extras):
            for dependency in optional.get(extra, []):
                pending.append(
                    (
                        str(dependency["name"]),
                        _dependency_extras(dependency),
                    )
                )

    selected.discard(ROOT_DISTRIBUTION)
    return {name: packages[name] for name in sorted(selected)}


def _distribution_metadata_directory(
    distribution: importlib.metadata.Distribution,
) -> tuple[PurePosixPath, Path]:
    for item in distribution.files or ():
        relative = PurePosixPath(str(item).replace("\\", "/"))
        if relative.name == "METADATA" and relative.parent.name.endswith(".dist-info"):
            source = Path(distribution.locate_file(item)).resolve(strict=True).parent
            return relative.parent, source
    raise LicenseBundleError(
        f"{distribution.metadata.get('Name', '<unknown>')} has no dist-info METADATA"
    )


def _safe_relative(value: str | PurePosixPath) -> PurePosixPath:
    relative = PurePosixPath(str(value).replace("\\", "/"))
    if relative.is_absolute() or not relative.parts or ".." in relative.parts:
        raise LicenseBundleError(f"unsafe archive path: {value}")
    return relative


def _is_legal_filename(path: PurePosixPath) -> bool:
    return (
        _LEGAL_NAME.search(path.name) is not None
        and path.suffix.lower() not in _NON_LEGAL_SUFFIXES
    )


def _record_file(
    source: Path,
    archive_root: Path,
    relative: PurePosixPath,
    *,
    kind: str,
    origin: str,
    records: dict[str, dict[str, Any]],
) -> str:
    relative = _safe_relative(relative)
    if source.is_symlink() or not source.is_file():
        raise LicenseBundleError(
            f"legal metadata source is not a regular file: {origin}"
        )
    target = archive_root.joinpath(*relative.parts)
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, target)
    path = relative.as_posix()
    record = {
        "path": path,
        "sha256": _file_digest(target),
        "size": target.stat().st_size,
        "kind": kind,
        "origin": origin,
    }
    previous = records.get(path)
    if previous is not None and previous != record:
        raise LicenseBundleError(f"conflicting compliance archive path: {path}")
    records[path] = record
    return path


def _metadata_value(metadata: Any, key: str) -> str | None:
    value = metadata.get(key)
    return str(value) if value is not None else None


def _effective_license(name: str, metadata: Any) -> str | None:
    if name == "openbb" or name.startswith("openbb-"):
        return "AGPL-3.0-only"
    expression = _metadata_value(metadata, "License-Expression")
    if expression:
        return expression.strip()
    legacy = _metadata_value(metadata, "License")
    if legacy and "\n" not in legacy and len(legacy) <= 200:
        return legacy.strip()
    classifiers = metadata.get_all("Classifier") or []
    if "License :: OSI Approved :: MIT License" in classifiers:
        return "MIT"
    return None


def _declared_license_matches(
    files: list[tuple[PurePosixPath, Path]], declared: str
) -> list[PurePosixPath]:
    normalized = PurePosixPath(declared.replace("\\", "/").lstrip("/"))
    matches = []
    for relative, source in files:
        if not source.is_file():
            continue
        if (
            relative == normalized
            or relative.as_posix().endswith(f"/licenses/{normalized.as_posix()}")
            or relative.as_posix().endswith(f"/{normalized.as_posix()}")
        ):
            matches.append(relative)
    return matches


def _fallback_file(
    name: str,
    effective_license: str | None,
    project_root: Path,
) -> tuple[Path, str, str] | None:
    if name == "openbb" or name.startswith("openbb-"):
        repository_license = project_root.parents[2] / "LICENSE"
        return repository_license, "LICENSE-AGPL-3.0-only.txt", "repository:LICENSE"
    if name in _PACKAGE_OVERRIDES:
        filename, origin = _PACKAGE_OVERRIDES[name]
        return project_root / "license-overrides" / filename, filename, origin
    if effective_license == "Apache-2.0":
        return (
            project_root / "license-overrides" / "Apache-2.0.txt",
            "LICENSE-Apache-2.0.txt",
            "https://www.apache.org/licenses/LICENSE-2.0.txt",
        )
    return None


def _archive_distribution(
    name: str,
    locked: dict[str, Any],
    distribution: importlib.metadata.Distribution,
    project_root: Path,
    archive_root: Path,
    records: dict[str, dict[str, Any]],
) -> dict[str, Any]:
    version = distribution.version
    if version != str(locked.get("version", "")):
        raise LicenseBundleError(
            f"{name} version {version} differs from uv.lock {locked.get('version')}"
        )
    metadata = distribution.metadata
    metadata_relative, metadata_source = _distribution_metadata_directory(distribution)
    package_key = f"{name}-{version}"
    archive_files: list[str] = []
    legal_files: set[str] = set()
    original_to_archive: dict[str, str] = {}

    for source in sorted(metadata_source.rglob("*")):
        if source.is_symlink():
            raise LicenseBundleError(f"symlink in {metadata_relative}: {source.name}")
        if not source.is_file() or source.name == "direct_url.json":
            continue
        tail = _safe_relative(source.relative_to(metadata_source).as_posix())
        destination = PurePosixPath("metadata") / package_key / tail
        original = (metadata_relative / tail).as_posix()
        archived = _record_file(
            source,
            archive_root,
            destination,
            kind="distribution-metadata",
            origin=f"distribution:{name}:{original}",
            records=records,
        )
        archive_files.append(archived)
        original_to_archive[original] = archived
        if _is_legal_filename(tail):
            legal_files.add(archived)

    installed_files: list[tuple[PurePosixPath, Path]] = []
    for item in distribution.files or ():
        relative = PurePosixPath(str(item).replace("\\", "/"))
        source = Path(distribution.locate_file(item)).resolve(strict=False)
        installed_files.append((relative, source))

    declared_records = []
    declared_paths: set[str] = set()
    for declared in metadata.get_all("License-File") or []:
        matches = _declared_license_matches(installed_files, str(declared))
        if not matches:
            raise LicenseBundleError(
                f"{name} declares an unavailable License-File: {declared}"
            )
        archived_matches = []
        for relative in matches:
            relative = _safe_relative(relative)
            original = relative.as_posix()
            archived = original_to_archive.get(original)
            if archived is None:
                source = next(
                    source
                    for candidate, source in installed_files
                    if candidate == relative
                )
                archived = _record_file(
                    source,
                    archive_root,
                    PurePosixPath("files") / package_key / relative,
                    kind="declared-license",
                    origin=f"distribution:{name}:{original}",
                    records=records,
                )
                archive_files.append(archived)
                original_to_archive[original] = archived
            legal_files.add(archived)
            declared_paths.add(original)
            archived_matches.append(archived)
        declared_records.append(
            {"declared": str(declared), "files": sorted(set(archived_matches))}
        )

    metadata_prefix = metadata_relative.as_posix() + "/"
    for relative, source in installed_files:
        if relative.is_absolute() or ".." in relative.parts:
            continue
        original = relative.as_posix()
        if original.startswith(metadata_prefix) or original in declared_paths:
            continue
        if not _is_legal_filename(relative) or not source.is_file():
            continue
        archived = _record_file(
            source,
            archive_root,
            PurePosixPath("files") / package_key / relative,
            kind="package-legal-file",
            origin=f"distribution:{name}:{original}",
            records=records,
        )
        archive_files.append(archived)
        legal_files.add(archived)

    effective_license = _effective_license(name, metadata)
    if not legal_files:
        fallback = _fallback_file(name, effective_license, project_root)
        if fallback is None:
            raise LicenseBundleError(
                f"{name} {version} has no license/NOTICE file or audited override"
            )
        source, filename, origin = fallback
        archived = _record_file(
            source,
            archive_root,
            PurePosixPath("overrides") / package_key / filename,
            kind="audited-license-override",
            origin=origin,
            records=records,
        )
        archive_files.append(archived)
        legal_files.add(archived)

    legacy_license = _metadata_value(metadata, "License")
    legacy_digest = (
        hashlib.sha256(legacy_license.encode("utf-8")).hexdigest()
        if legacy_license is not None
        else None
    )
    source_archive = locked.get("sdist")
    return {
        "name": name,
        "version": version,
        "effective_license": effective_license,
        "license_expression": _metadata_value(metadata, "License-Expression"),
        "legacy_license_sha256": legacy_digest,
        "license_classifiers": sorted(
            value
            for value in metadata.get_all("Classifier") or []
            if str(value).startswith("License ::")
        ),
        "project_urls": sorted(metadata.get_all("Project-URL") or []),
        "source_archive": source_archive,
        "metadata_directory": f"metadata/{package_key}",
        "declared_license_files": declared_records,
        "license_files": sorted(legal_files),
        "archive_files": sorted(set(archive_files)),
    }


def _archive_toolchains(
    archive_root: Path,
    records: dict[str, dict[str, Any]],
) -> list[dict[str, Any]]:
    python_version = platform.python_version()
    base_prefix = Path(sys.base_prefix)
    python_license = next(
        (
            candidate
            for candidate in (
                base_prefix / "LICENSE.txt",
                base_prefix / "LICENSE",
                base_prefix / "Lib" / "LICENSE.txt",
                base_prefix
                / "lib"
                / f"python{sys.version_info.major}.{sys.version_info.minor}"
                / "LICENSE.txt",
            )
            if candidate.is_file()
        ),
        None,
    )
    if python_license is None:
        raise LicenseBundleError("CPython license file is missing")
    python_path = _record_file(
        python_license,
        archive_root,
        PurePosixPath("toolchain") / f"cpython-{python_version}" / "LICENSE.txt",
        kind="toolchain-license",
        origin="CPython:LICENSE.txt",
        records=records,
    )

    pyinstaller = importlib.metadata.distribution("pyinstaller")
    metadata_relative, metadata_source = _distribution_metadata_directory(pyinstaller)
    pyinstaller_files = []
    pyinstaller_legal = []
    prefix = PurePosixPath("toolchain") / f"pyinstaller-{pyinstaller.version}"
    for source in sorted(metadata_source.rglob("*")):
        if source.is_symlink():
            raise LicenseBundleError("PyInstaller metadata contains a symlink")
        if not source.is_file() or source.name == "direct_url.json":
            continue
        tail = _safe_relative(source.relative_to(metadata_source).as_posix())
        archived = _record_file(
            source,
            archive_root,
            prefix / "metadata" / tail,
            kind="toolchain-metadata",
            origin=f"distribution:pyinstaller:{metadata_relative / tail}",
            records=records,
        )
        pyinstaller_files.append(archived)
        if _is_legal_filename(tail):
            pyinstaller_legal.append(archived)
    if not pyinstaller_legal:
        raise LicenseBundleError("PyInstaller license metadata is missing")
    return [
        {
            "name": "cpython",
            "version": python_version,
            "license_files": [python_path],
            "archive_files": [python_path],
        },
        {
            "name": "pyinstaller",
            "version": pyinstaller.version,
            "license_files": sorted(pyinstaller_legal),
            "archive_files": sorted(pyinstaller_files),
        },
    ]


def build_license_archive(project_root: Path, archive_root: Path) -> dict[str, Any]:
    """Collect the complete runtime compliance archive beside the frozen binary."""

    project_root = project_root.resolve(strict=True)
    lock_path = project_root / "uv.lock"
    lock_bytes = lock_path.read_bytes()
    locked_packages = locked_runtime_closure(lock_path)
    if archive_root.exists():
        shutil.rmtree(archive_root)
    archive_root.mkdir(parents=True)

    records: dict[str, dict[str, Any]] = {}
    distributions = []
    for name, locked in locked_packages.items():
        try:
            distribution = importlib.metadata.distribution(name)
        except importlib.metadata.PackageNotFoundError:
            # uv.lock includes packages for other resolution markers. Only the
            # distributions installed for this target can enter the bundle.
            continue
        distributions.append(
            _archive_distribution(
                name,
                locked,
                distribution,
                project_root,
                archive_root,
                records,
            )
        )

    manifest = {
        "schema_version": SCHEMA_VERSION,
        "uv_lock_sha256": hashlib.sha256(lock_bytes).hexdigest(),
        "platform": {
            "python": platform.python_version(),
            "system": sys.platform,
            "machine": platform.machine().lower(),
        },
        "distributions": sorted(distributions, key=lambda item: item["name"]),
        "toolchain": _archive_toolchains(archive_root, records),
        "files": [records[path] for path in sorted(records)],
    }
    manifest_path = archive_root / MANIFEST_NAME
    manifest_path.write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    validate_license_archive(archive_root, expected_lock=lock_bytes)
    return manifest


def validate_license_archive(
    archive_root: Path, *, expected_lock: bytes | None = None
) -> dict[str, Any]:
    """Fail closed if an archive file, package entry, or digest is incomplete."""

    if archive_root.is_symlink() or not archive_root.is_dir():
        raise LicenseBundleError("OpenBB third-party license archive is missing")
    manifest_path = archive_root / MANIFEST_NAME
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise LicenseBundleError("OpenBB license manifest is invalid") from error
    if manifest.get("schema_version") != SCHEMA_VERSION:
        raise LicenseBundleError("OpenBB license manifest schema is invalid")
    if (
        expected_lock is not None
        and manifest.get("uv_lock_sha256") != hashlib.sha256(expected_lock).hexdigest()
    ):
        raise LicenseBundleError("OpenBB license manifest uv.lock digest is invalid")

    file_records = manifest.get("files")
    if not isinstance(file_records, list) or not file_records:
        raise LicenseBundleError("OpenBB license manifest has no files")
    expected_paths: set[str] = set()
    for record in file_records:
        if not isinstance(record, dict):
            raise LicenseBundleError("OpenBB license file record is invalid")
        relative = _safe_relative(str(record.get("path", "")))
        path = archive_root.joinpath(*relative.parts)
        if path.is_symlink() or not path.is_file():
            raise LicenseBundleError(f"OpenBB license file is missing: {relative}")
        if path.stat().st_size != record.get("size") or _file_digest(
            path
        ) != record.get("sha256"):
            raise LicenseBundleError(
                f"OpenBB license file digest is invalid: {relative}"
            )
        if relative.name == "direct_url.json":
            raise LicenseBundleError("OpenBB license archive contains direct_url.json")
        if relative.as_posix() in expected_paths:
            raise LicenseBundleError(f"duplicate OpenBB license path: {relative}")
        expected_paths.add(relative.as_posix())

    actual_paths = set()
    for path in archive_root.rglob("*"):
        if path.is_symlink():
            raise LicenseBundleError(f"symlink in OpenBB license archive: {path.name}")
        if path.is_file() and path != manifest_path:
            actual_paths.add(path.relative_to(archive_root).as_posix())
    if actual_paths != expected_paths:
        raise LicenseBundleError("OpenBB license archive file inventory differs")

    distributions = manifest.get("distributions")
    if not isinstance(distributions, list) or len(distributions) < 100:
        raise LicenseBundleError("OpenBB runtime distribution inventory is incomplete")
    names = [entry.get("name") for entry in distributions if isinstance(entry, dict)]
    if names != sorted(names) or len(names) != len(set(names)):
        raise LicenseBundleError("OpenBB runtime distribution names are invalid")
    required = {"certifi", "cryptography", "numpy", "openbb", "openbb-core", "scipy"}
    if not required.issubset(set(names)):
        raise LicenseBundleError("OpenBB license archive omits a required distribution")
    for entry in distributions:
        archive_files = entry.get("archive_files")
        license_files = entry.get("license_files")
        if (
            not isinstance(archive_files, list)
            or not archive_files
            or not isinstance(license_files, list)
            or not license_files
            or not set(archive_files).issubset(expected_paths)
            or not set(license_files).issubset(expected_paths)
        ):
            raise LicenseBundleError(
                f"OpenBB distribution license inventory is incomplete: {entry.get('name')}"
            )
        for declared in entry.get("declared_license_files", []):
            if not declared.get("files") or not set(declared["files"]).issubset(
                expected_paths
            ):
                raise LicenseBundleError(
                    f"declared License-File is unresolved: {entry.get('name')}"
                )

    toolchain = manifest.get("toolchain")
    if not isinstance(toolchain, list) or {
        entry.get("name") for entry in toolchain if isinstance(entry, dict)
    } != {"cpython", "pyinstaller"}:
        raise LicenseBundleError("OpenBB toolchain license inventory is incomplete")
    for entry in toolchain:
        if not entry.get("license_files") or not set(entry["license_files"]).issubset(
            expected_paths
        ):
            raise LicenseBundleError(
                f"OpenBB toolchain license is incomplete: {entry.get('name')}"
            )
    return manifest
