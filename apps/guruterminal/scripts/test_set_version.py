#!/usr/bin/env python3
"""Exercise cross-manifest version updates in disposable repository fixtures."""

from __future__ import annotations

import json
import plistlib
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPTS = Path(__file__).resolve().parent
SET_VERSION = SCRIPTS / "set-version.py"


def write_text(path: Path, contents: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(contents, encoding="utf-8")


def write_json(path: Path, value: object) -> None:
    write_text(path, json.dumps(value, indent=2) + "\n")


def write_cargo_lock(path: Path, package_name: str, version: str) -> None:
    write_text(
        path,
        "\n".join(
            [
                "version = 4",
                "",
                "[[package]]",
                f'name = "{package_name}"',
                f'version = "{version}"',
                "",
            ]
        ),
    )


def write_package_lock(path: Path, name: str, version: str) -> None:
    write_json(
        path,
        {
            "name": name,
            "version": version,
            "lockfileVersion": 3,
            "requires": True,
            "packages": {"": {"name": name, "version": version}},
        },
    )


def seed_repository(root: Path, version: str = "0.0.1") -> None:
    base = version.split("-", 1)[0]
    write_text(
        root / "Cargo.toml",
        "\n".join(
            [
                "[package]",
                'name = "guruterminal-core"',
                f'version = "{version}"',
                'edition = "2021"',
                "",
            ]
        ),
    )
    write_cargo_lock(root / "Cargo.lock", "guruterminal-core", version)

    desktop = root / "apps/guruterminal"
    write_json(desktop / "package.json", {"name": "desktop", "version": version})
    write_package_lock(desktop / "package-lock.json", "desktop", version)
    write_json(
        desktop / "compute/package.json", {"name": "compute", "version": version}
    )
    write_package_lock(desktop / "compute/package-lock.json", "compute", version)
    write_text(
        desktop / "src-tauri/Cargo.toml",
        "\n".join(
            [
                "[package]",
                'name = "guruterminal-desktop"',
                f'version = "{version}"',
                'edition = "2021"',
                "",
            ]
        ),
    )
    write_cargo_lock(desktop / "src-tauri/Cargo.lock", "guruterminal-desktop", version)
    write_json(
        desktop / "src-tauri/tauri.conf.json",
        {"productName": "Guru Terminal", "version": version},
    )
    plist_path = desktop / "src-tauri/Info.plist"
    plist_path.parent.mkdir(parents=True, exist_ok=True)
    with plist_path.open("wb") as output:
        plistlib.dump({"CFBundleShortVersionString": base}, output)


def tracked_contents(root: Path) -> dict[Path, bytes]:
    return {
        path.relative_to(root): path.read_bytes()
        for path in root.rglob("*")
        if path.is_file()
    }


class SetVersionTest(unittest.TestCase):
    def run_script(
        self, root: Path, *arguments: str
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SET_VERSION), "--root", str(root), *arguments],
            check=False,
            text=True,
            capture_output=True,
        )

    def assert_product_version(self, root: Path, version: str) -> None:
        desktop = root / "apps/guruterminal"
        self.assertIn(f'version = "{version}"', (root / "Cargo.toml").read_text())
        self.assertIn(
            f'version = "{version}"',
            (desktop / "src-tauri/Cargo.toml").read_text(),
        )
        self.assertIn(f'version = "{version}"', (root / "Cargo.lock").read_text())
        self.assertIn(
            f'version = "{version}"',
            (desktop / "src-tauri/Cargo.lock").read_text(),
        )
        for path in [desktop / "package.json", desktop / "compute/package.json"]:
            self.assertEqual(json.loads(path.read_text())["version"], version)
        for path in [
            desktop / "package-lock.json",
            desktop / "compute/package-lock.json",
        ]:
            lock = json.loads(path.read_text())
            self.assertEqual(lock["version"], version)
            self.assertEqual(lock["packages"][""]["version"], version)
        self.assertEqual(
            json.loads((desktop / "src-tauri/tauri.conf.json").read_text())["version"],
            version,
        )
        with (desktop / "src-tauri/Info.plist").open("rb") as source:
            self.assertEqual(
                plistlib.load(source)["CFBundleShortVersionString"],
                version.split("-", 1)[0],
            )

    def test_updates_an_rc_then_stable_version_and_validates_both(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            seed_repository(root)

            candidate = "1.2.3-rc.4"
            completed = self.run_script(root, "--version", candidate)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assert_product_version(root, candidate)

            checked = self.run_script(root, "--check", "--version", candidate)
            self.assertEqual(checked.returncode, 0, checked.stderr)

            stable = "1.2.3"
            completed = self.run_script(root, "--version", stable)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assert_product_version(root, stable)

    def test_dry_run_leaves_the_repository_unchanged(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            seed_repository(root)
            before = tracked_contents(root)

            completed = self.run_script(root, "--version", "1.2.3-rc.1", "--dry-run")

            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertIn("apps/guruterminal/src-tauri/Info.plist", completed.stdout)
            self.assertEqual(tracked_contents(root), before)

    def test_rejects_invalid_versions_without_writing(self) -> None:
        for version in ("1.2", "01.2.3", "1.2.3-beta.1", "1.2.3-rc.0"):
            with (
                self.subTest(version=version),
                tempfile.TemporaryDirectory() as temporary_directory,
            ):
                root = Path(temporary_directory)
                seed_repository(root)
                before = tracked_contents(root)

                completed = self.run_script(root, "--version", version)

                self.assertNotEqual(completed.returncode, 0)
                self.assertIn("canonical", completed.stderr)
                self.assertEqual(tracked_contents(root), before)

    def test_refuses_to_overwrite_inconsistent_checked_in_versions(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            seed_repository(root)
            package_path = root / "apps/guruterminal/compute/package.json"
            package = json.loads(package_path.read_text())
            package["version"] = "0.0.2"
            write_json(package_path, package)
            before = tracked_contents(root)

            completed = self.run_script(root, "--version", "1.2.3")

            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("versions disagree", completed.stderr)
            self.assertEqual(tracked_contents(root), before)


if __name__ == "__main__":
    unittest.main()
