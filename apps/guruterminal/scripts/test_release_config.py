#!/usr/bin/env python3
"""Exercise the release-only Tauri configuration with disposable fixtures."""

from __future__ import annotations

import json
import os
import plistlib
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPTS = Path(__file__).resolve().parent
CREATE_RELEASE_CONFIG = SCRIPTS / "create-release-config.py"
CHECK_MACOS_BUNDLE_VERSION = SCRIPTS / "check-macos-bundle-version.py"
NEXT_MACOS_BUNDLE_VERSION = SCRIPTS / "next-macos-bundle-version.py"
STABLE_ENDPOINT = (
    "https://github.com/monarchjuno/guruterminal/releases/latest/download/latest.json"
)
NEXT_RC_ENDPOINT = (
    "https://github.com/monarchjuno/guruterminal/releases/download/"
    "v1.2.3-rc.5/latest.json"
)


class ReleaseConfigTest(unittest.TestCase):
    def write_release_config(self, directory: Path) -> Path:
        path = directory / "tauri.release.conf.json"
        path.write_text(
            json.dumps(
                {
                    "bundle": {"createUpdaterArtifacts": True},
                    "plugins": {"updater": {"windows": {"installMode": "passive"}}},
                }
            ),
            encoding="utf-8",
        )
        return path

    def run_create_release_config(
        self,
        directory: Path,
        platform: str,
        *extra_arguments: str,
        version: str = "1.2.3",
        old_update_endpoints: str = '["https://attacker.example.invalid/latest.json"]',
    ) -> subprocess.CompletedProcess[str]:
        environment = os.environ.copy()
        environment.update(
            {
                "GURUTERMINAL_UPDATER_PUBLIC_KEY": "fixture-updater-public-key",
                "GURUTERMINAL_UPDATE_ENDPOINTS": old_update_endpoints,
                "GURUTERMINAL_WINDOWS_CERTIFICATE_THUMBPRINT": "A" * 40,
                "GURUTERMINAL_WINDOWS_TIMESTAMP_URL": "https://timestamp.example.test",
            }
        )
        return subprocess.run(
            [
                sys.executable,
                str(CREATE_RELEASE_CONFIG),
                "--platform",
                platform,
                "--version",
                version,
                *extra_arguments,
                "--release-config",
                str(self.write_release_config(directory)),
                "--output",
                str(directory / "generated.json"),
            ],
            check=False,
            capture_output=True,
            text=True,
            env=environment,
        )

    def test_release_version_is_required(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            directory = Path(temporary_directory)
            completed = subprocess.run(
                [
                    sys.executable,
                    str(CREATE_RELEASE_CONFIG),
                    "--platform",
                    "windows",
                    "--release-config",
                    str(self.write_release_config(directory)),
                    "--output",
                    str(directory / "generated.json"),
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("--version", completed.stderr)

    def test_stable_release_derives_the_canonical_update_endpoint(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            directory = Path(temporary_directory)
            completed = self.run_create_release_config(
                directory,
                "macos",
                "--macos-bundle-version",
                "17",
                version="1.2.3",
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            config = json.loads((directory / "generated.json").read_text())
            self.assertEqual(config["bundle"]["macOS"]["bundleVersion"], "17")
            self.assertEqual(
                config["plugins"]["updater"]["endpoints"], [STABLE_ENDPOINT]
            )

    def test_rc_release_derives_the_next_rc_then_stable_endpoints(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            directory = Path(temporary_directory)
            completed = self.run_create_release_config(
                directory,
                "windows",
                version="1.2.3-rc.4",
                old_update_endpoints=json.dumps([STABLE_ENDPOINT]),
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            config = json.loads((directory / "generated.json").read_text())
            self.assertEqual(
                config["plugins"]["updater"]["endpoints"],
                [NEXT_RC_ENDPOINT, STABLE_ENDPOINT],
            )

    def test_malicious_legacy_endpoint_environment_is_ignored(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            directory = Path(temporary_directory)
            completed = self.run_create_release_config(
                directory,
                "windows",
                version="1.2.3-rc.4",
                old_update_endpoints="not even valid JSON",
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            config = json.loads((directory / "generated.json").read_text())
            self.assertEqual(
                config["plugins"]["updater"]["endpoints"],
                [NEXT_RC_ENDPOINT, STABLE_ENDPOINT],
            )

    def test_release_version_rejects_noncanonical_versions(self) -> None:
        for version in (
            "1.2",
            "01.2.3",
            "1.2.3-beta.1",
            "1.2.3-rc.0",
            "1.2.3-rc.١",
            "1.2.3+build.1",
        ):
            with (
                self.subTest(version=version),
                tempfile.TemporaryDirectory() as temporary_directory,
            ):
                completed = self.run_create_release_config(
                    Path(temporary_directory),
                    "windows",
                    version=version,
                )
                self.assertNotEqual(completed.returncode, 0)
                self.assertIn("--version", completed.stderr)

    def test_macos_release_counter_is_required_and_canonical(self) -> None:
        for value in (None, "", "0", "-1", "01", "1.2", "counter"):
            with (
                self.subTest(value=value),
                tempfile.TemporaryDirectory() as temporary_directory,
            ):
                arguments = () if value is None else ("--macos-bundle-version", value)
                completed = self.run_create_release_config(
                    Path(temporary_directory), "macos", *arguments
                )
                self.assertNotEqual(completed.returncode, 0)
                self.assertIn("--macos-bundle-version", completed.stderr)

    def test_windows_overlay_does_not_accept_a_macos_build_counter(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            directory = Path(temporary_directory)
            completed = self.run_create_release_config(directory, "windows")
            self.assertEqual(completed.returncode, 0, completed.stderr)
            config = json.loads((directory / "generated.json").read_text())
            self.assertNotIn("macOS", config["bundle"])

            rejected = self.run_create_release_config(
                directory, "windows", "--macos-bundle-version", "17"
            )
            self.assertNotEqual(rejected.returncode, 0)
            self.assertIn("valid only for macos", rejected.stderr)


class MacosBundleVersionCheckTest(unittest.TestCase):
    def run_check(
        self, directory: Path, *extra_arguments: str
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                sys.executable,
                str(CHECK_MACOS_BUNDLE_VERSION),
                "--tauri-config",
                str(directory / "tauri.conf.json"),
                "--source-plist",
                str(directory / "Info.plist"),
                "--bundle-plist",
                str(directory / "packaged-Info.plist"),
                *extra_arguments,
            ],
            check=False,
            capture_output=True,
            text=True,
        )

    def test_packaged_release_build_uses_the_explicit_counter(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            directory = Path(temporary_directory)
            (directory / "tauri.conf.json").write_text(
                json.dumps(
                    {
                        "version": "0.0.1-rc.1",
                        "bundle": {"macOS": {"bundleVersion": "1"}},
                    }
                ),
                encoding="utf-8",
            )
            for name, value in (
                ("Info.plist", {"CFBundleShortVersionString": "0.0.1"}),
                (
                    "packaged-Info.plist",
                    {
                        "CFBundleShortVersionString": "0.0.1",
                        "CFBundleVersion": "17",
                    },
                ),
            ):
                with (directory / name).open("wb") as output:
                    plistlib.dump(value, output)

            default = self.run_check(directory)
            self.assertNotEqual(default.returncode, 0)
            self.assertIn("expected macOS build version", default.stderr)

            matched = self.run_check(directory, "--expected-bundle-version", "17")
            self.assertEqual(matched.returncode, 0, matched.stderr)

            mismatched = self.run_check(directory, "--expected-bundle-version", "18")
            self.assertNotEqual(mismatched.returncode, 0)
            self.assertIn("expected macOS build version", mismatched.stderr)


class NextMacosBundleVersionTest(unittest.TestCase):
    def run_allocator(
        self, metadata_directory: Path, run_number: str
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                sys.executable,
                str(NEXT_MACOS_BUNDLE_VERSION),
                "--run-number",
                run_number,
                "--release-metadata-directory",
                str(metadata_directory),
            ],
            check=False,
            capture_output=True,
            text=True,
        )

    def write_metadata(
        self, metadata_directory: Path, name: str, bundle_version: str
    ) -> None:
        path = metadata_directory / name
        path.mkdir(parents=True)
        (path / "RELEASE-METADATA.json").write_text(
            json.dumps(
                {
                    "schema_version": 2,
                    "macos_bundle_version": bundle_version,
                }
            ),
            encoding="utf-8",
        )

    def test_uses_the_run_number_when_no_prior_release_exists(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            metadata_directory = Path(temporary_directory) / "metadata"
            metadata_directory.mkdir()
            completed = self.run_allocator(metadata_directory, "17")
            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertEqual(completed.stdout, "17\n")

    def test_advances_beyond_every_retained_release_counter(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            metadata_directory = Path(temporary_directory) / "metadata"
            self.write_metadata(metadata_directory, "older-rc", "17")
            self.write_metadata(metadata_directory, "newer-stable", "53")
            completed = self.run_allocator(metadata_directory, "42")
            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertEqual(completed.stdout, "54\n")

    def test_rejects_unversioned_or_invalid_retained_metadata(self) -> None:
        for metadata in (
            {"schema_version": 1, "macos_bundle_version": "1"},
            {"schema_version": 2, "macos_bundle_version": "01"},
        ):
            with (
                self.subTest(metadata=metadata),
                tempfile.TemporaryDirectory() as temporary_directory,
            ):
                metadata_directory = Path(temporary_directory) / "metadata"
                release_directory = metadata_directory / "release"
                release_directory.mkdir(parents=True)
                (release_directory / "RELEASE-METADATA.json").write_text(
                    json.dumps(metadata), encoding="utf-8"
                )
                completed = self.run_allocator(metadata_directory, "17")
                self.assertNotEqual(completed.returncode, 0)
                self.assertIn("release metadata", completed.stderr)


if __name__ == "__main__":
    unittest.main()
