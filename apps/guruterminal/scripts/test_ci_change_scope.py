#!/usr/bin/env python3
"""Unit tests for CI change-scope classification."""

from __future__ import annotations

import runpy
import unittest
from pathlib import Path


SCRIPTS = Path(__file__).resolve().parent
SCOPE = runpy.run_path(str(SCRIPTS / "ci_change_scope.py"))
classify = SCOPE["classify"]


class ChangeScopeTests(unittest.TestCase):
    def test_frontend_only_runs_native_and_skips_packaging(self) -> None:
        native, packaging = classify(["apps/guruterminal/src/App.tsx"])
        self.assertTrue(native)
        self.assertFalse(packaging)

    def test_vite_config_is_native_not_packaging(self) -> None:
        native, packaging = classify(["apps/guruterminal/vite.config.ts"])
        self.assertTrue(native)
        self.assertFalse(packaging)

    def test_docs_only_skips_native_and_packaging(self) -> None:
        native, packaging = classify(["docs/ci-cd.md", "README.md", "CHANGELOG.md"])
        self.assertFalse(native)
        self.assertFalse(packaging)

    def test_tauri_changes_run_native_and_packaging(self) -> None:
        native, packaging = classify(["apps/guruterminal/src-tauri/src/lib.rs"])
        self.assertTrue(native)
        self.assertTrue(packaging)

    def test_core_sidecar_runs_native_and_packaging(self) -> None:
        native, packaging = classify(["src/lib.rs"])
        self.assertTrue(native)
        self.assertTrue(packaging)

    def test_ci_workflow_forces_the_full_set(self) -> None:
        native, packaging = classify([".github/workflows/ci.yml", "docs/ci-cd.md"])
        self.assertTrue(native)
        self.assertTrue(packaging)

    def test_setup_action_forces_the_full_set(self) -> None:
        native, packaging = classify([".github/actions/setup-toolchains/action.yml"])
        self.assertTrue(native)
        self.assertTrue(packaging)

    def test_change_scope_script_forces_the_full_set(self) -> None:
        native, packaging = classify(["apps/guruterminal/scripts/ci_change_scope.py"])
        self.assertTrue(native)
        self.assertTrue(packaging)

    def test_force_full_overrides_docs_only(self) -> None:
        native, packaging = classify(["docs/ci-cd.md"], force_full=True)
        self.assertTrue(native)
        self.assertTrue(packaging)

    def test_mixed_frontend_and_docs_still_skips_packaging(self) -> None:
        native, packaging = classify(
            ["apps/guruterminal/src/main.tsx", "docs/architecture.md"]
        )
        self.assertTrue(native)
        self.assertFalse(packaging)

    def test_stage_script_runs_native_and_packaging(self) -> None:
        native, packaging = classify(["apps/guruterminal/scripts/stage-macos-arm64.sh"])
        self.assertTrue(native)
        self.assertTrue(packaging)

    def test_agent_resource_runs_both(self) -> None:
        native, packaging = classify(["apps/guruterminal/agent/SYSTEM.md"])
        self.assertTrue(native)
        self.assertTrue(packaging)

    def test_empty_change_set_skips_expensive_jobs(self) -> None:
        native, packaging = classify([])
        self.assertFalse(native)
        self.assertFalse(packaging)


if __name__ == "__main__":
    unittest.main()
