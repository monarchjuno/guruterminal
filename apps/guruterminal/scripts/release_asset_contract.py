#!/usr/bin/env python3
"""Shared, non-security vocabulary for release-asset producer scripts.

The independent release verifier, updater-signature verifier, qualification
workflow, and packaged runtime intentionally retain their own explicit asset
expectations. This module only keeps the producers from accidentally drifting
from one another while they assemble a candidate.
"""

from __future__ import annotations


RELEASE_METADATA_SCHEMA = 2
RELEASE_METADATA_NAME = "RELEASE-METADATA.json"
SHA256SUMS_NAME = "SHA256SUMS"
UPDATER_MANIFEST_NAME = "latest.json"

MACOS_UPDATER_PLATFORM = "darwin-aarch64"
WINDOWS_UPDATER_PLATFORM = "windows-x86_64"

MACOS_INSTALLER_ALIAS = "GuruTerminal-macOS-arm64.dmg"
WINDOWS_INSTALLER_ALIAS = "GuruTerminal-Windows-x64.exe"

METADATA_SCHEMA_VERSION = "schema_version"
METADATA_REPOSITORY = "repository"
METADATA_TAG = "tag"
METADATA_VERSION = "version"
METADATA_SOURCE_COMMIT = "source_commit"
METADATA_WORKFLOW_RUN_ID = "workflow_run_id"
METADATA_MACOS_BUNDLE_VERSION = "macos_bundle_version"
METADATA_UPDATER_MANIFEST = "updater_manifest"
METADATA_ARTIFACTS = "artifacts"
METADATA_DOWNLOAD_ALIASES = "download_aliases"


def macos_installer_name(version: str) -> str:
    return f"Guru Terminal-{version}-aarch64-apple-darwin.dmg"


def macos_updater_name(version: str) -> str:
    return f"Guru Terminal-{version}-darwin-aarch64.app.tar.gz"


def windows_updater_name(version: str) -> str:
    return f"Guru Terminal-{version}-x86_64-pc-windows-msvc-setup.exe"


def updater_signature_name(artifact_name: str) -> str:
    return f"{artifact_name}.sig"


def sbom_name(version: str) -> str:
    return f"Guru Terminal-{version}.spdx.json"


def updater_artifact_names(version: str) -> dict[str, str]:
    return {
        MACOS_UPDATER_PLATFORM: macos_updater_name(version),
        WINDOWS_UPDATER_PLATFORM: windows_updater_name(version),
    }


def canonical_asset_names(version: str) -> list[str]:
    macos_updater = macos_updater_name(version)
    windows_updater = windows_updater_name(version)
    return [
        macos_installer_name(version),
        macos_updater,
        updater_signature_name(macos_updater),
        windows_updater,
        updater_signature_name(windows_updater),
        sbom_name(version),
        UPDATER_MANIFEST_NAME,
    ]


def download_aliases(version: str) -> dict[str, str]:
    return {
        MACOS_INSTALLER_ALIAS: macos_installer_name(version),
        WINDOWS_INSTALLER_ALIAS: windows_updater_name(version),
    }
