#!/usr/bin/env python3
"""Exercise the release asset assembly contract with disposable fixtures."""

from __future__ import annotations

import hashlib
import json
import runpy
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from release_asset_contract import (  # noqa: E402
    canonical_asset_names,
    download_aliases,
    updater_artifact_names,
)


SCRIPTS = Path(__file__).resolve().parent
REPOSITORY_ROOT = SCRIPTS.parents[2]
VERSION = "0.0.1"
TAG = f"v{VERSION}"
REPOSITORY = "monarchjuno/guruterminal"
SOURCE_COMMIT = "0123456789abcdef0123456789abcdef01234567"
WORKFLOW_RUN_ID = "42"
MACOS_BUNDLE_VERSION = "17"
PUBLISHED_AT = "2026-08-25T00:00:00Z"
SERVE_UPDATE_CANDIDATE = runpy.run_path(str(SCRIPTS / "serve-update-candidate.py"))


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class ReleaseAssetsContractTest(unittest.TestCase):
    def run_script(
        self, script: str, *arguments: str
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPTS / script), *arguments],
            check=False,
            text=True,
            capture_output=True,
        )

    def assert_script_succeeds(
        self, script: str, *arguments: str
    ) -> subprocess.CompletedProcess[str]:
        completed = self.run_script(script, *arguments)
        self.assertEqual(
            completed.returncode,
            0,
            f"{script} failed:\nstdout:\n{completed.stdout}\nstderr:\n{completed.stderr}",
        )
        return completed

    def seed_unsigned_candidate(self, assets: Path) -> None:
        assets.mkdir()
        fixture_files = {
            f"Guru Terminal-{VERSION}-aarch64-apple-darwin.dmg": b"fixture dmg\n",
            f"Guru Terminal-{VERSION}-darwin-aarch64.app.tar.gz": b"fixture mac updater\n",
            f"Guru Terminal-{VERSION}-darwin-aarch64.app.tar.gz.sig": b"fixture mac signature\n",
            f"Guru Terminal-{VERSION}-x86_64-pc-windows-msvc-setup.exe": b"fixture windows updater\n",
            f"Guru Terminal-{VERSION}-x86_64-pc-windows-msvc-setup.exe.sig": b"fixture windows signature\n",
        }
        for name, contents in fixture_files.items():
            (assets / name).write_bytes(contents)
        (assets / f"Guru Terminal-{VERSION}.spdx.json").write_text(
            json.dumps({"spdxVersion": "SPDX-2.3", "name": "fixture"}),
            encoding="utf-8",
        )

    def assemble_candidate(self, assets: Path) -> None:
        self.seed_unsigned_candidate(assets)
        self.assert_script_succeeds(
            "generate-update-manifest.py",
            "--version",
            VERSION,
            "--tag",
            TAG,
            "--repository",
            REPOSITORY,
            "--pub-date",
            PUBLISHED_AT,
            "--assets",
            str(assets),
            "--output",
            str(assets / "latest.json"),
        )
        self.assert_script_succeeds(
            "finalize-release-assets.py",
            "--version",
            VERSION,
            "--tag",
            TAG,
            "--repository",
            REPOSITORY,
            "--source-commit",
            SOURCE_COMMIT,
            "--workflow-run-id",
            WORKFLOW_RUN_ID,
            "--macos-bundle-version",
            MACOS_BUNDLE_VERSION,
            "--assets",
            str(assets),
        )

    def verify_candidate(self, assets: Path) -> subprocess.CompletedProcess[str]:
        return self.assert_script_succeeds(
            "verify-release-assets.py",
            "--version",
            VERSION,
            "--tag",
            TAG,
            "--repository",
            REPOSITORY,
            "--source-commit",
            SOURCE_COMMIT,
            "--assets",
            str(assets),
        )

    def rewrite_checksums(self, assets: Path) -> None:
        names = sorted(
            path.name for path in assets.iterdir() if path.name != "SHA256SUMS"
        )
        (assets / "SHA256SUMS").write_text(
            "".join(f"{sha256(assets / name)}  {name}\n" for name in names),
            encoding="utf-8",
        )

    def test_shared_vocabulary_matches_the_published_asset_contract(self) -> None:
        macos_updater = f"Guru Terminal-{VERSION}-darwin-aarch64.app.tar.gz"
        windows_updater = f"Guru Terminal-{VERSION}-x86_64-pc-windows-msvc-setup.exe"
        self.assertEqual(
            canonical_asset_names(VERSION),
            [
                f"Guru Terminal-{VERSION}-aarch64-apple-darwin.dmg",
                macos_updater,
                f"{macos_updater}.sig",
                windows_updater,
                f"{windows_updater}.sig",
                f"Guru Terminal-{VERSION}.spdx.json",
                "latest.json",
            ],
        )
        self.assertEqual(
            updater_artifact_names(VERSION),
            {
                "darwin-aarch64": macos_updater,
                "windows-x86_64": windows_updater,
            },
        )
        self.assertEqual(
            download_aliases(VERSION),
            {
                "GuruTerminal-macOS-arm64.dmg": (
                    f"Guru Terminal-{VERSION}-aarch64-apple-darwin.dmg"
                ),
                "GuruTerminal-Windows-x64.exe": windows_updater,
            },
        )

    def test_prepare_normalizes_macos_and_windows_bundle_outputs(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            temporary = Path(temporary_directory)
            macos_bundle = temporary / "macos-bundle"
            macos_output = temporary / "macos-assets"
            (macos_bundle / "dmg").mkdir(parents=True)
            (macos_bundle / "macos").mkdir()
            (macos_bundle / "dmg" / "fixture.dmg").write_bytes(b"fixture dmg\n")
            (macos_bundle / "macos" / "fixture.app.tar.gz").write_bytes(
                b"fixture mac updater\n"
            )
            (macos_bundle / "macos" / "fixture.app.tar.gz.sig").write_bytes(
                b"fixture mac signature\n"
            )
            self.assert_script_succeeds(
                "prepare-release-assets.py",
                "--platform",
                "macos",
                "--version",
                VERSION,
                "--bundle-root",
                str(macos_bundle),
                "--output",
                str(macos_output),
            )
            self.assertEqual(
                {path.name for path in macos_output.iterdir()},
                set(canonical_asset_names(VERSION)[:3]),
            )

            windows_bundle = temporary / "windows-bundle"
            windows_output = temporary / "windows-assets"
            (windows_bundle / "nsis").mkdir(parents=True)
            (windows_bundle / "nsis" / "fixture-setup.exe").write_bytes(
                b"fixture windows updater\n"
            )
            (windows_bundle / "nsis" / "fixture-setup.exe.sig").write_bytes(
                b"fixture windows signature\n"
            )
            self.assert_script_succeeds(
                "prepare-release-assets.py",
                "--platform",
                "windows",
                "--version",
                VERSION,
                "--bundle-root",
                str(windows_bundle),
                "--output",
                str(windows_output),
            )
            self.assertEqual(
                {path.name for path in windows_output.iterdir()},
                set(canonical_asset_names(VERSION)[3:5]),
            )

    def test_candidate_server_routes_the_shared_updater_asset_names(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            assets = Path(temporary_directory) / "release-assets"
            self.assemble_candidate(assets)
            route_map = SERVE_UPDATE_CANDIDATE["routes"](assets, REPOSITORY)
            prefix = f"/{REPOSITORY}/releases"
            resolved_assets = assets.resolve()
            expected = {
                f"{prefix}/latest/download/latest.json": (
                    resolved_assets / "latest.json"
                ),
            } | {
                f"{prefix}/download/{TAG}/{name}": resolved_assets / name
                for name in updater_artifact_names(VERSION).values()
            }
            self.assertEqual(route_map, expected)

    def test_generate_finalize_and_verify_a_complete_candidate(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            assets = Path(temporary_directory) / "release-assets"
            self.assemble_candidate(assets)
            self.verify_candidate(assets)

            manifest = json.loads((assets / "latest.json").read_text(encoding="utf-8"))
            self.assertEqual(manifest["version"], VERSION)
            self.assertEqual(
                manifest["platforms"]["darwin-aarch64"]["signature"],
                "fixture mac signature",
            )
            self.assertEqual(
                manifest["platforms"]["windows-x86_64"]["signature"],
                "fixture windows signature",
            )

            metadata = json.loads(
                (assets / "RELEASE-METADATA.json").read_text(encoding="utf-8")
            )
            self.assertEqual(metadata["source_commit"], SOURCE_COMMIT)
            self.assertEqual(metadata["macos_bundle_version"], MACOS_BUNDLE_VERSION)
            self.assertEqual(
                (assets / "GuruTerminal-macOS-arm64.dmg").read_bytes(),
                (
                    assets / f"Guru Terminal-{VERSION}-aarch64-apple-darwin.dmg"
                ).read_bytes(),
            )
            self.assertEqual(
                (assets / "GuruTerminal-Windows-x64.exe").read_bytes(),
                (
                    assets / f"Guru Terminal-{VERSION}-x86_64-pc-windows-msvc-setup.exe"
                ).read_bytes(),
            )

    def test_verifier_rejects_a_tampered_updater_url_after_checksums_are_repaired(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            assets = Path(temporary_directory) / "release-assets"
            self.assemble_candidate(assets)

            manifest_path = assets / "latest.json"
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            manifest["platforms"]["darwin-aarch64"]["url"] = (
                "https://example.invalid/incorrect-updater.tar.gz"
            )
            manifest_path.write_text(
                json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
                encoding="utf-8",
            )

            metadata_path = assets / "RELEASE-METADATA.json"
            metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
            metadata["artifacts"]["latest.json"]["sha256"] = sha256(manifest_path)
            metadata_path.write_text(
                json.dumps(metadata, ensure_ascii=False, indent=2, sort_keys=True)
                + "\n",
                encoding="utf-8",
            )
            self.rewrite_checksums(assets)

            completed = self.run_script(
                "verify-release-assets.py",
                "--version",
                VERSION,
                "--tag",
                TAG,
                "--repository",
                REPOSITORY,
                "--source-commit",
                SOURCE_COMMIT,
                "--assets",
                str(assets),
            )
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn(
                "updater manifest URLs or signatures do not match the assets",
                completed.stderr,
            )

    def test_qualification_receipt_seals_the_verified_candidate_set(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            assets = Path(temporary_directory) / "release-assets"
            receipt = Path(temporary_directory) / "qualification.json"
            self.assemble_candidate(assets)
            candidate_digest = self.assert_script_succeeds(
                "release-qualification.py",
                "digest",
                "--release-metadata",
                str(assets / "RELEASE-METADATA.json"),
            ).stdout.strip()
            self.assert_script_succeeds(
                "release-qualification.py",
                "write",
                "--candidate-tag",
                TAG,
                "--previous-tag",
                "v0.0.1-rc.1",
                "--repository",
                REPOSITORY,
                "--source-commit",
                SOURCE_COMMIT,
                "--release-id",
                "99",
                "--release-metadata",
                str(assets / "RELEASE-METADATA.json"),
                "--workflow-run-id",
                "100",
                "--macos-evidence-url",
                "https://evidence.example.test/macos",
                "--macos-candidate-set-sha256",
                candidate_digest,
                "--windows-evidence-url",
                "https://evidence.example.test/windows",
                "--windows-candidate-set-sha256",
                candidate_digest,
                "--macos-product-acceptance-evidence-url",
                "https://evidence.example.test/macos-product-acceptance",
                "--windows-product-acceptance-evidence-url",
                "https://evidence.example.test/windows-product-acceptance",
                "--output",
                str(receipt),
            )
            sealed = json.loads(receipt.read_text(encoding="utf-8"))
            self.assertEqual(sealed["schema_version"], 2)
            self.assertEqual(
                sealed["product_acceptance"]["macos-aarch64"],
                {
                    "result": "passed",
                    "evidence_url": "https://evidence.example.test/macos-product-acceptance",
                    "candidate_set_sha256": candidate_digest,
                },
            )
            self.assert_script_succeeds(
                "release-qualification.py",
                "verify",
                "--candidate-tag",
                TAG,
                "--repository",
                REPOSITORY,
                "--source-commit",
                SOURCE_COMMIT,
                "--release-id",
                "99",
                "--release-metadata",
                str(assets / "RELEASE-METADATA.json"),
                "--workflow-run-id",
                "100",
                "--receipt",
                str(receipt),
            )
            sealed["product_acceptance"]["windows-x86_64"]["candidate_set_sha256"] = (
                "0" * 64
            )
            receipt.write_text(
                json.dumps(sealed, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
            completed = self.run_script(
                "release-qualification.py",
                "verify",
                "--candidate-tag",
                TAG,
                "--repository",
                REPOSITORY,
                "--source-commit",
                SOURCE_COMMIT,
                "--release-id",
                "99",
                "--release-metadata",
                str(assets / "RELEASE-METADATA.json"),
                "--workflow-run-id",
                "100",
                "--receipt",
                str(receipt),
            )
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("product acceptance candidate-set digest", completed.stderr)

    def test_promotion_requires_the_candidate_tag_workflow_ref(self) -> None:
        workflow = (
            REPOSITORY_ROOT / ".github/workflows/promote-release.yml"
        ).read_text(encoding="utf-8")
        self.assertIn('test "$GITHUB_REF_TYPE" = tag', workflow)
        self.assertIn('test "$GITHUB_REF_NAME" = "$CANDIDATE_TAG"', workflow)
        self.assertIn('git rev-parse "$GITHUB_SHA^{commit}"', workflow)

    def test_qualification_seals_signed_product_acceptance_evidence(self) -> None:
        workflow = (
            REPOSITORY_ROOT / ".github/workflows/release-qualification.yml"
        ).read_text(encoding="utf-8")
        for required in (
            "macos_product_acceptance_evidence_url:",
            "windows_product_acceptance_evidence_url:",
            "confirm_macos_product_acceptance:",
            "confirm_windows_product_acceptance:",
            'test "$CONFIRM_MACOS_PRODUCT_ACCEPTANCE" = true',
            'test "$CONFIRM_WINDOWS_PRODUCT_ACCEPTANCE" = true',
            "--macos-product-acceptance-evidence-url",
            "--windows-product-acceptance-evidence-url",
        ):
            self.assertIn(required, workflow)

    def test_stable_qualification_sequence_requires_latest_after_first_release(
        self,
    ) -> None:
        self.assert_script_succeeds(
            "release-qualification.py",
            "sequence",
            "--candidate-tag",
            "v0.0.1",
            "--previous-tag",
            "v0.0.1-rc.1",
            "--latest-stable-tag",
            "",
        )
        self.assert_script_succeeds(
            "release-qualification.py",
            "sequence",
            "--candidate-tag",
            "v0.0.1",
            "--previous-tag",
            "v0.0.0",
            "--latest-stable-tag",
            "v0.0.0",
        )

        rc_after_stable = self.run_script(
            "release-qualification.py",
            "sequence",
            "--candidate-tag",
            "v0.0.1",
            "--previous-tag",
            "v0.0.1-rc.1",
            "--latest-stable-tag",
            "v0.0.0",
        )
        self.assertNotEqual(rc_after_stable.returncode, 0)
        self.assertIn(
            "must start from the current Latest release", rc_after_stable.stderr
        )

        non_monotonic = self.run_script(
            "release-qualification.py",
            "sequence",
            "--candidate-tag",
            "v0.0.1",
            "--previous-tag",
            "v0.0.0",
            "--latest-stable-tag",
            "v0.0.1",
        )
        self.assertNotEqual(non_monotonic.returncode, 0)
        self.assertIn("must be strictly newer", non_monotonic.stderr)

    def test_release_candidates_must_form_a_contiguous_update_line(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            published = Path(temporary_directory) / "published-tags.txt"

            def validate(
                candidate_tag: str, tags: list[str]
            ) -> subprocess.CompletedProcess[str]:
                published.write_text("\n".join(tags) + "\n", encoding="utf-8")
                return self.run_script(
                    "release-qualification.py",
                    "rc-sequence",
                    "--candidate-tag",
                    candidate_tag,
                    "--published-tags",
                    str(published),
                )

            self.assertEqual(validate("v1.2.3-rc.1", []).returncode, 0)
            self.assertEqual(
                validate("v1.2.3-rc.2", ["v1.2.3-rc.1", "v9.9.9-rc.7"]).returncode,
                0,
            )

            skipped = validate("v1.2.3-rc.3", ["v1.2.3-rc.1"])
            self.assertNotEqual(skipped.returncode, 0)
            self.assertIn("expected rc.2", skipped.stderr)

            broken_history = validate("v1.2.3-rc.4", ["v1.2.3-rc.1", "v1.2.3-rc.3"])
            self.assertNotEqual(broken_history.returncode, 0)
            self.assertIn("must be contiguous", broken_history.stderr)

            after_stable = validate("v1.2.3-rc.1", ["v1.2.3"])
            self.assertNotEqual(after_stable.returncode, 0)
            self.assertIn("after its matching stable", after_stable.stderr)

            malformed = validate("v1.2.3-rc.0", [])
            self.assertNotEqual(malformed.returncode, 0)
            self.assertIn("canonical", malformed.stderr)

    def test_release_workflow_enforces_contiguous_rc_candidates(self) -> None:
        workflow = (REPOSITORY_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("Require contiguous release candidates", workflow)
        self.assertIn("release-qualification.py rc-sequence", workflow)
        self.assertIn("published-release-tags.txt", workflow)


if __name__ == "__main__":
    unittest.main()
