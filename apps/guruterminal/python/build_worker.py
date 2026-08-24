"""Create the production one-directory worker bundle."""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path
from tempfile import TemporaryDirectory


def remove_local_build_metadata(bundle_root: Path) -> None:
    """Do not ship PEP 610 records containing the build machine's source path."""
    for metadata in bundle_root.rglob("direct_url.json"):
        if metadata.parent.name.endswith(".dist-info"):
            metadata.unlink()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--codesign-identity",
        help="Optional macOS code-signing identity passed to PyInstaller.",
    )
    parser.add_argument("--distpath", default="dist")
    arguments = parser.parse_args()

    root = Path(__file__).resolve().parent
    dist_root = root / arguments.distpath
    with TemporaryDirectory(prefix="guruterminal-pyinstaller-") as temporary:
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
            "--name",
            "guruterminal-finance",
            "--paths",
            str(root / "src"),
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

    bundle_root = dist_root / "guruterminal-finance"
    remove_local_build_metadata(bundle_root)
    executable = bundle_root
    executable /= (
        "guruterminal-finance.exe"
        if sys.platform == "win32"
        else "guruterminal-finance"
    )
    if not executable.is_file():
        raise SystemExit(f"PyInstaller did not create {executable}")
    print(executable)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
