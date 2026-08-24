#!/usr/bin/env python3
"""Unit tests for the read-only GitHub release-setup audit."""

from __future__ import annotations

import runpy
import tempfile
import unittest
from pathlib import Path


SCRIPTS = Path(__file__).resolve().parent
AUDIT = runpy.run_path(str(SCRIPTS / "check-github-release-setup.py"))
audit_release_setup = AUDIT["audit_release_setup"]
required_release_secret_names = AUDIT["required_release_secret_names"]


REPOSITORY = "monarchjuno/guruterminal"
REQUIRED_SECRETS = {
    "GURUTERMINAL_UPDATER_PUBLIC_KEY",
    "GURUTERMINAL_WINDOWS_CERTIFICATE",
}


def configured_rulesets() -> list[dict[str, object]]:
    return [
        {
            "target": "branch",
            "enforcement": "active",
            "conditions": {"ref_name": {"include": ["~DEFAULT_BRANCH"]}},
            "rules": [{"type": "required_status_checks"}],
        },
        {
            "target": "tag",
            "enforcement": "active",
            "conditions": {"ref_name": {"include": ["refs/tags/v*"]}},
            "rules": [{"type": "creation"}],
        },
    ]


def configured_environments() -> dict[str, object]:
    return {
        "environments": [
            {"name": "release", "protection_rules": [{"id": 1}]},
            {"name": "release-qualification", "protection_rules": [{"id": 2}]},
            {"name": "stable-release", "protection_rules": [{"id": 3}]},
        ]
    }


class GitHubReleaseSetupTest(unittest.TestCase):
    def audit(self, **overrides: object):
        defaults: dict[str, object] = {
            "repository": REPOSITORY,
            "release_secrets": REQUIRED_SECRETS,
            "required_secrets": REQUIRED_SECRETS,
            "repository_data": {
                "full_name": REPOSITORY,
                "visibility": "public",
                "private": False,
                "default_branch": "main",
            },
            "rulesets": configured_rulesets(),
            "main_has_legacy_protection": False,
            "environments": configured_environments(),
        }
        defaults.update(overrides)
        return audit_release_setup(**defaults)

    def test_configured_repository_passes_every_required_check(self) -> None:
        findings = self.audit()
        self.assertEqual([finding.level for finding in findings], ["pass"] * 8)

    def test_missing_protection_environments_and_secrets_are_actionable(self) -> None:
        findings = self.audit(
            rulesets=[],
            environments={"environments": []},
            release_secrets=set(),
        )
        errors = {
            finding.subject: finding.detail
            for finding in findings
            if finding.level == "error"
        }
        self.assertIn("main protection", errors)
        self.assertIn("release tags", errors)
        self.assertIn("environment release", errors)
        self.assertIn("environment release-qualification", errors)
        self.assertIn("environment stable-release", errors)
        self.assertEqual(
            errors["release environment secrets"],
            "missing workflow-referenced secret names: "
            "GURUTERMINAL_UPDATER_PUBLIC_KEY, GURUTERMINAL_WINDOWS_CERTIFICATE",
        )

    def test_disabled_or_empty_rulesets_do_not_count_as_protection(self) -> None:
        findings = self.audit(
            rulesets=[
                {
                    "target": "branch",
                    "enforcement": "disabled",
                    "conditions": {"ref_name": {"include": ["~DEFAULT_BRANCH"]}},
                    "rules": [{"type": "required_status_checks"}],
                },
                {
                    "target": "tag",
                    "enforcement": "active",
                    "conditions": {"ref_name": {"include": ["refs/tags/v*"]}},
                    "rules": [],
                },
            ]
        )
        errors = {finding.subject for finding in findings if finding.level == "error"}
        self.assertEqual(
            errors & {"main protection", "release tags"},
            {"main protection", "release tags"},
        )

    def test_branch_policy_counts_as_environment_protection(self) -> None:
        environments = configured_environments()
        environments["environments"] = [
            {
                "name": "release",
                "deployment_branch_policy": {"protected_branches": True},
            },
            {"name": "release-qualification", "protection_rules": [{"id": 2}]},
            {"name": "stable-release", "protection_rules": [{"id": 3}]},
        ]
        findings = self.audit(environments=environments)
        self.assertNotIn("error", [finding.level for finding in findings])

    def test_workflow_secret_references_are_derived_without_reading_values(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            workflow = Path(temporary_directory) / "release.yml"
            workflow.write_text(
                "env:\n  TOKEN: ${{ secrets.GURUTERMINAL_ONE }}\n"
                "  OTHER: ${{secrets.GURUTERMINAL_TWO}}\n",
                encoding="utf-8",
            )
            self.assertEqual(
                required_release_secret_names(workflow),
                {"GURUTERMINAL_ONE", "GURUTERMINAL_TWO"},
            )


if __name__ == "__main__":
    unittest.main()
