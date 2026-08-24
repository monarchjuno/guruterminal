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
            "rules": [{"type": "pull_request"}],
        },
        {
            "target": "tag",
            "enforcement": "active",
            "conditions": {"ref_name": {"include": ["refs/tags/v*"]}},
            "rules": [
                {"type": "creation"},
                {"type": "update"},
                {"type": "deletion"},
            ],
        },
    ]


def configured_environments() -> dict[str, object]:
    return {
        "environments": [
            {"name": "release", "protection_rules": [{"id": 1}]},
            {"name": "release-qualification", "protection_rules": [{"id": 2}]},
            {
                "name": "stable-release",
                "protection_rules": [
                    {
                        "type": "required_reviewers",
                        "prevent_self_review": True,
                        "reviewers": [
                            {"type": "User", "reviewer": {"id": 1, "login": "reviewer"}}
                        ],
                    }
                ],
            },
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
            "main_legacy_protection": None,
            "immutable_releases": {"enabled": True, "enforced_by_owner": False},
            "environments": configured_environments(),
        }
        defaults.update(overrides)
        return audit_release_setup(**defaults)

    def test_configured_repository_passes_every_required_check(self) -> None:
        findings = self.audit()
        self.assertEqual([finding.level for finding in findings], ["pass"] * 9)

    def test_missing_protection_environments_and_secrets_are_actionable(self) -> None:
        findings = self.audit(
            rulesets=[],
            environments={"environments": []},
            release_secrets=set(),
            immutable_releases={"enabled": False, "enforced_by_owner": False},
        )
        errors = {
            finding.subject: finding.detail
            for finding in findings
            if finding.level == "error"
        }
        self.assertIn("main protection", errors)
        self.assertIn("release tags", errors)
        self.assertIn("immutable releases", errors)
        self.assertIn("environment release", errors)
        self.assertIn("environment release-qualification", errors)
        self.assertIn("environment stable-release", errors)
        self.assertEqual(
            errors["release environment secrets"],
            "missing workflow-referenced secret names: "
            "GURUTERMINAL_UPDATER_PUBLIC_KEY, GURUTERMINAL_WINDOWS_CERTIFICATE",
        )

    def test_repository_owner_immutable_release_enforcement_counts_as_enabled(
        self,
    ) -> None:
        findings = self.audit(
            immutable_releases={"enabled": False, "enforced_by_owner": True}
        )
        self.assertNotIn("error", [finding.level for finding in findings])

    def test_disabled_immutable_releases_are_actionable(self) -> None:
        findings = self.audit(
            immutable_releases={"enabled": False, "enforced_by_owner": False}
        )
        errors = {finding.subject for finding in findings if finding.level == "error"}
        self.assertIn("immutable releases", errors)

    def test_disabled_or_incomplete_rulesets_do_not_count_as_protection(self) -> None:
        findings = self.audit(
            rulesets=[
                {
                    "target": "branch",
                    "enforcement": "disabled",
                    "conditions": {"ref_name": {"include": ["~DEFAULT_BRANCH"]}},
                    "rules": [{"type": "pull_request"}],
                },
                {
                    "target": "tag",
                    "enforcement": "active",
                    "conditions": {"ref_name": {"include": ["refs/tags/v*"]}},
                    "rules": [{"type": "creation"}],
                },
            ]
        )
        errors = {finding.subject for finding in findings if finding.level == "error"}
        self.assertEqual(
            errors & {"main protection", "release tags"},
            {"main protection", "release tags"},
        )

    def test_legacy_main_protection_must_require_a_pull_request(self) -> None:
        rulesets = configured_rulesets()[1:]
        findings = self.audit(
            rulesets=rulesets,
            main_legacy_protection={
                "required_pull_request_reviews": {"required_approving_review_count": 1}
            },
        )
        self.assertNotIn("error", [finding.level for finding in findings])

        findings = self.audit(
            rulesets=rulesets,
            main_legacy_protection={"required_status_checks": {"strict": True}},
        )
        errors = {finding.subject for finding in findings if finding.level == "error"}
        self.assertIn("main protection", errors)

    def test_branch_policy_counts_as_environment_protection(self) -> None:
        environments = configured_environments()
        environments["environments"] = [
            {
                "name": "release",
                "deployment_branch_policy": {"protected_branches": True},
            },
            {"name": "release-qualification", "protection_rules": [{"id": 2}]},
            configured_environments()["environments"][2],
        ]
        findings = self.audit(environments=environments)
        self.assertNotIn("error", [finding.level for finding in findings])

    def test_stable_release_requires_an_independent_reviewer(self) -> None:
        environments = configured_environments()
        environments["environments"][2] = {
            "name": "stable-release",
            "protection_rules": [{"type": "wait_timer", "wait_timer": 5}],
        }
        findings = self.audit(environments=environments)
        errors = {
            finding.subject: finding.detail
            for finding in findings
            if finding.level == "error"
        }
        self.assertEqual(
            errors["environment stable-release"],
            "configure a required reviewer and prevent self-review",
        )

    def test_stable_release_reviewer_cannot_self_approve(self) -> None:
        environments = configured_environments()
        environments["environments"][2] = {
            "name": "stable-release",
            "protection_rules": [
                {
                    "type": "required_reviewers",
                    "prevent_self_review": False,
                    "reviewers": [{"type": "User", "reviewer": {"id": 1}}],
                }
            ],
        }
        findings = self.audit(environments=environments)
        errors = {
            finding.subject: finding.detail
            for finding in findings
            if finding.level == "error"
        }
        self.assertEqual(
            errors["environment stable-release"],
            "configure a required reviewer and prevent self-review",
        )

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
