from __future__ import annotations

import hashlib
import os
from pathlib import Path

import pytest

from guruterminal_openbb.materialize import materialize
from license_bundle import (
    ARCHIVE_DIRECTORY,
    LicenseBundleError,
    build_license_archive,
    validate_license_archive,
)


def test_pyinstaller_hook_collects_random_user_agent_runtime_data() -> None:
    project_root = Path(__file__).resolve().parents[1]
    hook = (project_root / "hooks" / "hook-openbb.py").read_text(encoding="utf-8")

    assert '"random_user_agent": "random-user-agent"' in hook


@pytest.mark.skipif(os.name == "nt", reason="Windows staging has no bundle symlinks")
def test_materialize_replaces_internal_symlink(tmp_path: Path) -> None:
    target = tmp_path / "library.dylib"
    target.write_bytes(b"library")
    link = tmp_path / "alias.dylib"
    link.symlink_to(target.name)

    materialize(tmp_path)

    assert not link.is_symlink()
    assert link.read_bytes() == b"library"


@pytest.mark.skipif(os.name == "nt", reason="Windows staging has no bundle symlinks")
def test_materialize_replaces_internal_directory_symlink(tmp_path: Path) -> None:
    target = tmp_path / "framework-resources"
    target.mkdir()
    (target / "library").write_bytes(b"directory library")
    link = tmp_path / "Resources"
    link.symlink_to(target.name, target_is_directory=True)

    materialize(tmp_path)

    assert not link.is_symlink()
    assert (link / "library").read_bytes() == b"directory library"


@pytest.mark.skipif(os.name == "nt", reason="Windows staging has no bundle symlinks")
def test_materialize_rejects_escaping_symlink(tmp_path: Path) -> None:
    outside = tmp_path.parent / f"{tmp_path.name}-outside"
    outside.write_bytes(b"outside")
    link = tmp_path / "escape"
    link.symlink_to(outside)

    with pytest.raises(SystemExit, match="escapes its bundle"):
        materialize(tmp_path)


def test_runtime_license_archive_covers_metadata_and_toolchains(
    tmp_path: Path,
) -> None:
    project_root = Path(__file__).resolve().parents[1]
    archive_root = tmp_path / ARCHIVE_DIRECTORY
    manifest = build_license_archive(project_root, archive_root)
    validate_license_archive(
        archive_root,
        expected_lock=(project_root / "uv.lock").read_bytes(),
    )

    distributions = {entry["name"]: entry for entry in manifest["distributions"]}
    assert len(distributions) >= 190
    assert set(distributions) >= {
        "certifi",
        "cryptography",
        "numpy",
        "openbb",
        "openbb-core",
        "openbb-mcp-server",
        "scipy",
    }
    assert len(distributions["numpy"]["declared_license_files"]) >= 17
    assert len(distributions["numpy"]["license_files"]) >= 17
    assert len(distributions["scipy"]["license_files"]) >= 5
    assert len(distributions["cryptography"]["license_files"]) >= 3
    assert distributions["certifi"]["license_files"]

    repository_license_digest = hashlib.sha256(
        (project_root.parents[2] / "LICENSE").read_bytes()
    ).hexdigest()
    records = {record["path"]: record for record in manifest["files"]}
    openbb_entries = [
        entry
        for name, entry in distributions.items()
        if name == "openbb" or name.startswith("openbb-")
    ]
    assert len(openbb_entries) >= 50
    assert all(
        entry["effective_license"] == "AGPL-3.0-only" for entry in openbb_entries
    )
    assert all(
        any(
            records[path]["sha256"] == repository_license_digest
            for path in entry["license_files"]
        )
        for entry in openbb_entries
    )
    assert {entry["name"] for entry in manifest["toolchain"]} == {
        "cpython",
        "pyinstaller",
    }
    assert all("direct_url.json" not in record["path"] for record in manifest["files"])

    certifi_license = archive_root / distributions["certifi"]["license_files"][0]
    certifi_license.write_bytes(certifi_license.read_bytes() + b"tampered")
    with pytest.raises(LicenseBundleError, match="digest is invalid"):
        validate_license_archive(archive_root)
