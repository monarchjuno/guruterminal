#!/usr/bin/env python3
"""Classify a CI change set so expensive native and package jobs can be skipped.

Pull requests that only touch documentation skip both native interaction and
package smoke. Renderer-only changes still run native interaction, because that
is the user-visible surface, but they skip installer packaging. ``main`` pushes
and ``workflow_dispatch`` always run the full set. Required GitHub check names
stay green by running the skipped jobs as cheap no-op steps.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from collections.abc import Iterable
from pathlib import Path


NATIVE_PREFIXES = (
    "apps/guruterminal/src/",
    "apps/guruterminal/e2e/",
    "apps/guruterminal/src-tauri/",
    "apps/guruterminal/agent/",
    "apps/guruterminal/marketplace/",
    "apps/guruterminal/python/",
    "apps/guruterminal/openbb/",
    "apps/guruterminal/compute/",
    "apps/guruterminal/public/",
    "src/",
)
NATIVE_EXACT = {
    "Cargo.toml",
    "Cargo.lock",
    "apps/guruterminal/package.json",
    "apps/guruterminal/package-lock.json",
    "apps/guruterminal/index.html",
    "apps/guruterminal/vite.config.ts",
    "apps/guruterminal/tsconfig.json",
    "apps/guruterminal/tsconfig.app.json",
    "apps/guruterminal/tsconfig.node.json",
    "apps/guruterminal/components.json",
}
NATIVE_NAME_PREFIXES = ("apps/guruterminal/scripts/stage-",)

PACKAGING_PREFIXES = (
    "apps/guruterminal/src-tauri/",
    "apps/guruterminal/python/",
    "apps/guruterminal/openbb/",
    "apps/guruterminal/agent/",
    "apps/guruterminal/compute/",
    "src/",
)
PACKAGING_EXACT = {
    "Cargo.toml",
    "Cargo.lock",
    "LICENSE",
    "NOTICE",
    "apps/guruterminal/THIRD_PARTY_NOTICES.md",
    "apps/guruterminal/scripts/check-sidecars.py",
}
PACKAGING_NAME_PREFIXES = (
    "apps/guruterminal/scripts/stage-",
    "apps/guruterminal/scripts/check-macos-",
    "apps/guruterminal/scripts/check-windows-",
    "apps/guruterminal/scripts/check-package-",
)

FORCE_FULL_EXACT = {
    ".github/workflows/ci.yml",
    "apps/guruterminal/scripts/ci_change_scope.py",
}
FORCE_FULL_PREFIXES = (".github/actions/",)


def normalize(path: str) -> str:
    normalized = path.replace("\\", "/")
    while normalized.startswith("./"):
        normalized = normalized[2:]
    return normalized


def _has_prefix(path: str, prefixes: Iterable[str]) -> bool:
    return any(path.startswith(prefix) for prefix in prefixes)


def forces_full(paths: Iterable[str]) -> bool:
    for path in paths:
        normalized = normalize(path)
        if normalized in FORCE_FULL_EXACT or _has_prefix(
            normalized, FORCE_FULL_PREFIXES
        ):
            return True
    return False


def is_native_path(path: str) -> bool:
    normalized = normalize(path)
    return (
        normalized in NATIVE_EXACT
        or _has_prefix(normalized, NATIVE_PREFIXES)
        or _has_prefix(normalized, NATIVE_NAME_PREFIXES)
    )


def is_packaging_path(path: str) -> bool:
    normalized = normalize(path)
    return (
        normalized in PACKAGING_EXACT
        or _has_prefix(normalized, PACKAGING_PREFIXES)
        or _has_prefix(normalized, PACKAGING_NAME_PREFIXES)
    )


def classify(paths: Iterable[str], *, force_full: bool = False) -> tuple[bool, bool]:
    path_list = [normalize(path) for path in paths if normalize(path)]
    if force_full or forces_full(path_list):
        return True, True
    native = any(is_native_path(path) for path in path_list)
    packaging = any(is_packaging_path(path) for path in path_list)
    return native, packaging


def git_changed_files(base_sha: str, head_sha: str) -> list[str]:
    completed = subprocess.run(
        [
            "git",
            "diff",
            "--name-only",
            "--diff-filter=ACMRTUXB",
            f"{base_sha}...{head_sha}",
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise RuntimeError(completed.stderr.strip() or "git diff failed")
    return [line.strip() for line in completed.stdout.splitlines() if line.strip()]


def write_github_output(native: bool, packaging: bool) -> None:
    payload = f"native={str(native).lower()}\npackaging={str(packaging).lower()}\n"
    print(payload, end="")
    output_path = os.environ.get("GITHUB_OUTPUT")
    if not output_path:
        return
    path = Path(output_path)
    with path.open("a", encoding="utf-8") as handle:
        handle.write(payload)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--force-full", action="store_true")
    parser.add_argument("--base-sha")
    parser.add_argument("--head-sha")
    arguments = parser.parse_args()
    force_full = arguments.force_full or os.environ.get("CI_FORCE_FULL") == "1"
    try:
        if force_full:
            paths: list[str] = []
        else:
            base_sha = arguments.base_sha or os.environ.get("CI_BASE_SHA", "")
            head_sha = arguments.head_sha or os.environ.get("CI_HEAD_SHA", "")
            if not base_sha or not head_sha:
                raise RuntimeError(
                    "pull_request classification needs base and head SHAs"
                )
            paths = git_changed_files(base_sha, head_sha)
        native, packaging = classify(paths, force_full=force_full)
    except Exception as error:
        print(
            f"change-scope classification failed ({error}); running the full CI set",
            file=sys.stderr,
        )
        native, packaging = True, True
    write_github_output(native, packaging)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
