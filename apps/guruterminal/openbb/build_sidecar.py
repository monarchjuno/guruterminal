"""Build and validate the production OpenBB one-directory sidecar."""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import os
import shutil
import subprocess
import sys
from pathlib import Path
from tempfile import TemporaryDirectory

from guruterminal_openbb.bootstrap import configure_scratch_environment
from guruterminal_openbb.manifest import load_runtime_manifest
from license_bundle import (
    ARCHIVE_DIRECTORY,
    MANIFEST_NAME as LICENSE_MANIFEST_NAME,
    build_license_archive,
)


def remove_local_build_metadata(bundle_root: Path) -> None:
    """Do not ship PEP 610 records containing the build machine source path."""

    for metadata in bundle_root.rglob("direct_url.json"):
        if metadata.parent.name.endswith(".dist-info"):
            metadata.unlink()


def _console_script(name: str) -> Path:
    suffix = ".exe" if sys.platform == "win32" else ""
    # Preserve the virtualenv path. Resolving the Python symlink would jump to
    # uv's shared interpreter and could fall back to an unrelated global CLI.
    candidate = Path(sys.executable).parent / f"{name}{suffix}"
    if not candidate.is_file():
        discovered = shutil.which(name)
        if discovered:
            candidate = Path(discovered)
    if not candidate.is_file():
        raise SystemExit(f"required console script is missing: {name}")
    return candidate


def validate_environment(root: Path) -> None:
    """Check package pins, generated OpenBB extensions, and provider coverage."""

    if sys.version_info[:2] != (3, 12):
        raise SystemExit("OpenBB sidecar must be built with Python 3.12")
    manifest = load_runtime_manifest(root / "runtime-manifest.json")
    if manifest.get("schema_version") != "guruterminal-mcp-runtime/1":
        raise SystemExit("unsupported OpenBB runtime manifest schema")
    lock_digest = hashlib.sha256((root / "uv.lock").read_bytes()).hexdigest()
    if manifest.get("uv_lock_sha256") != lock_digest:
        raise SystemExit("OpenBB uv.lock digest does not match runtime manifest")

    packages = manifest.get("packages", {})
    for package, expected in packages.items():
        actual = importlib.metadata.version(package)
        if actual != expected:
            raise SystemExit(f"{package} version {actual} does not match {expected}")

    provider_packages = {
        provider["package"]: provider["version"]
        for provider in manifest.get("providers", [])
    }
    for package, expected in provider_packages.items():
        actual = importlib.metadata.version(package)
        if actual != expected:
            raise SystemExit(f"{package} version {actual} does not match {expected}")

    from openbb import obb  # imported only after openbb-build

    discovered = set(obb.coverage.providers)
    declared = {provider["id"] for provider in manifest["providers"]}
    if discovered != declared:
        missing = sorted(discovered - declared)
        extra = sorted(declared - discovered)
        raise SystemExit(
            f"OpenBB provider manifest mismatch; missing={missing}, extra={extra}"
        )

    credential_fields = set(type(obb.user.credentials).model_fields)
    mapped_credentials = {
        field
        for provider in manifest["providers"]
        for field in provider.get("credential_mapping", {}).values()
    }
    mapped_credentials.update(
        field
        for provider in manifest["providers"]
        for field in provider.get("config_mapping", {}).values()
        if field != "sec_contact_email"
    )
    unknown_credentials = sorted(mapped_credentials - credential_fields)
    if unknown_credentials:
        raise SystemExit(
            "OpenBB manifest references unknown credential fields: "
            f"{unknown_credentials}"
        )


def run_openbb_build(root: Path) -> None:
    """Materialize OpenBB's dynamic extension package before freezing it."""

    completed = subprocess.run(
        [str(_console_script("openbb-build"))],
        cwd=root,
        check=False,
        env=os.environ.copy(),
    )
    if completed.returncode:
        raise SystemExit(completed.returncode)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--codesign-identity",
        help="Optional macOS code-signing identity passed to PyInstaller.",
    )
    parser.add_argument("--distpath", default="dist")
    parser.add_argument(
        "--check",
        action="store_true",
        help="Run openbb-build and validate the frozen inputs without PyInstaller.",
    )
    arguments = parser.parse_args()

    if sys.platform == "darwin":
        os.environ.setdefault("MACOSX_DEPLOYMENT_TARGET", "13.0")

    root = Path(__file__).resolve().parent
    build_home = TemporaryDirectory(prefix="guruterminal-openbb-build-home-")
    isolated_home = Path(build_home.name)
    isolated_home.chmod(0o700)
    configure_scratch_environment(isolated_home)
    try:
        run_openbb_build(root)
        validate_environment(root)
        if arguments.check:
            build_license_archive(root, isolated_home / ARCHIVE_DIRECTORY)
            print("OpenBB sidecar inputs are valid.")
            return 0

        dist_root = root / arguments.distpath
        with TemporaryDirectory(prefix="guruterminal-openbb-pyinstaller-") as temporary:
            build_root = Path(temporary)
            work_root = build_root / "work"
            spec_root = build_root / "spec"
            config_root = build_root / "config"
            spec_root.mkdir()
            config_root.mkdir()

            command = [
                sys.executable,
                "-m",
                "PyInstaller",
                "--clean",
                "--noconfirm",
                "--onedir",
                "--console",
                "--noupx",
                "--name",
                "guruterminal-openbb",
                "--paths",
                str(root / "src"),
                "--additional-hooks-dir",
                str(root / "hooks"),
                "--hidden-import",
                "openbb",
                "--hidden-import",
                "openbb_core.api.rest_api",
                "--hidden-import",
                "openbb_mcp_server.app.app",
                "--exclude-module",
                "pytest",
                "--exclude-module",
                "_pytest",
                "--exclude-module",
                "openbb_mcp_server.skills",
                "--add-data",
                f"{root / 'runtime-manifest.json'}:guruterminal_openbb",
                "--add-data",
                f"{root / 'uv.lock'}:.",
                "--distpath",
                str(dist_root),
                "--workpath",
                str(work_root),
                "--specpath",
                str(spec_root),
            ]
            if arguments.codesign_identity:
                command.extend(["--codesign-identity", arguments.codesign_identity])
            command.append(str(root / "pyinstaller_entrypoint.py"))

            environment = os.environ.copy()
            environment["PYINSTALLER_CONFIG_DIR"] = str(config_root)
            completed = subprocess.run(command, cwd=root, check=False, env=environment)
            if completed.returncode:
                return completed.returncode

        bundle_root = dist_root / "guruterminal-openbb"
        remove_local_build_metadata(bundle_root)
        shutil.copy2(
            root / "runtime-manifest.json", bundle_root / "runtime-manifest.json"
        )
        shutil.copy2(root / "uv.lock", bundle_root / "uv.lock")
        build_license_archive(root, bundle_root / ARCHIVE_DIRECTORY)
        executable = bundle_root / (
            "guruterminal-openbb.exe"
            if sys.platform == "win32"
            else "guruterminal-openbb"
        )
        packaged_manifest = (
            bundle_root / "_internal" / "guruterminal_openbb" / "runtime-manifest.json"
        )
        if not executable.is_file():
            raise SystemExit(f"PyInstaller did not create {executable}")
        if not packaged_manifest.is_file():
            raise SystemExit(f"PyInstaller did not package {packaged_manifest}")
        if not (bundle_root / "runtime-manifest.json").is_file():
            raise SystemExit("OpenBB bundle is missing its public runtime manifest")
        if not (bundle_root / ARCHIVE_DIRECTORY / LICENSE_MANIFEST_NAME).is_file():
            raise SystemExit("OpenBB bundle is missing its Python license manifest")
        print(executable)
        return 0
    finally:
        build_home.cleanup()


if __name__ == "__main__":
    raise SystemExit(main())
