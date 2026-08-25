#!/usr/bin/env python3
"""Unit tests for the read-only GitHub release-setup audit."""

from __future__ import annotations

import runpy
import tempfile
import unittest
from unittest import mock
from pathlib import Path


SCRIPTS = Path(__file__).resolve().parent
AUDIT = runpy.run_path(str(SCRIPTS / "check-github-release-setup.py"))
AuditError = AUDIT["AuditError"]
audit_release_setup = AUDIT["audit_release_setup"]
load_ruleset_details = AUDIT["load_ruleset_details"]
remote_audit = AUDIT["remote_audit"]
repository_secret_names = AUDIT["repository_secret_names"]
required_release_secret_names = AUDIT["required_release_secret_names"]


REPOSITORY = "monarchjuno/guruterminal"
REQUIRED_SECRETS = {
    "GURUTERMINAL_UPDATER_PUBLIC_KEY",
    "GURUTERMINAL_WINDOWS_CERTIFICATE",
}
GITHUB_ACTIONS_INTEGRATION_ID = 15368
MAIN_REQUIRED_CHECK_CONTEXTS = (
    "Source and product contracts",
    "Native macOS interaction",
    "Native Windows interaction",
    "Package smoke (aarch64-apple-darwin)",
    "Package smoke (x86_64-pc-windows-msvc)",
)


def required_ci_rule() -> dict[str, object]:
    return {
        "type": "required_status_checks",
        "parameters": {
            "do_not_enforce_on_create": False,
            "strict_required_status_checks_policy": True,
            "required_status_checks": [
                {"context": context, "integration_id": GITHUB_ACTIONS_INTEGRATION_ID}
                for context in MAIN_REQUIRED_CHECK_CONTEXTS
            ],
        },
    }


def legacy_required_ci_checks() -> dict[str, object]:
    return {
        "strict": True,
        "checks": [
            {"context": context, "app_id": GITHUB_ACTIONS_INTEGRATION_ID}
            for context in MAIN_REQUIRED_CHECK_CONTEXTS
        ],
    }


def configured_rulesets() -> list[dict[str, object]]:
    return [
        {
            "target": "branch",
            "enforcement": "active",
            "bypass_actors": [],
            "conditions": {"ref_name": {"include": ["~DEFAULT_BRANCH"]}},
            "rules": [
                {"type": "pull_request"},
                required_ci_rule(),
                {"type": "non_fast_forward"},
            ],
        },
        {
            "target": "tag",
            "enforcement": "active",
            "bypass_actors": [
                {
                    "actor_id": 1,
                    "actor_type": "User",
                    "bypass_mode": "always",
                }
            ],
            "conditions": {"ref_name": {"include": ["refs/tags/v*"]}},
            "rules": [{"type": "creation"}],
        },
        {
            "target": "tag",
            "enforcement": "active",
            "bypass_actors": [],
            "conditions": {"ref_name": {"include": ["refs/tags/v*"]}},
            "rules": [{"type": "update"}, {"type": "deletion"}],
        },
    ]


def configured_environments() -> dict[str, object]:
    return {
        "environments": [
            {
                "name": "release",
                "protection_rules": [{"id": 1}],
                "deployment_branch_policy": {
                    "protected_branches": False,
                    "custom_branch_policies": True,
                },
            },
            {
                "name": "release-qualification",
                "protection_rules": [{"id": 2}],
                "deployment_branch_policy": {
                    "protected_branches": False,
                    "custom_branch_policies": True,
                },
            },
            {
                "name": "stable-release",
                "can_admins_bypass": False,
                "protection_rules": [
                    {
                        "type": "required_reviewers",
                        "prevent_self_review": True,
                        "reviewers": [
                            {"type": "User", "reviewer": {"id": 1, "login": "reviewer"}}
                        ],
                    }
                ],
                "deployment_branch_policy": {
                    "protected_branches": False,
                    "custom_branch_policies": True,
                },
            },
        ]
    }


def configured_environment_secrets() -> dict[str, set[str]]:
    return {
        "release": set(REQUIRED_SECRETS),
        "release-qualification": set(),
        "stable-release": set(),
    }


def configured_deployment_branch_policies() -> dict[str, object]:
    return {
        name: {
            "total_count": 1,
            "branch_policies": [{"id": 1, "node_id": f"policy-{name}", "name": "v*"}],
        }
        for name in ("release", "release-qualification", "stable-release")
    }


def secret_response(names: set[str]) -> dict[str, object]:
    return {
        "total_count": len(names),
        "secrets": [{"name": name} for name in sorted(names)],
    }


class GitHubReleaseSetupTest(unittest.TestCase):
    def audit(self, **overrides: object):
        defaults: dict[str, object] = {
            "repository": REPOSITORY,
            "repository_secrets": set(),
            "environment_secrets": configured_environment_secrets(),
            "deployment_branch_policies": configured_deployment_branch_policies(),
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
        self.assertEqual([finding.level for finding in findings], ["pass"] * 12)
        environment_details = {
            finding.subject: finding.detail
            for finding in findings
            if finding.subject.startswith("environment ")
        }
        for name in ("release", "release-qualification", "stable-release"):
            self.assertIn(
                "GITHUB_REF_TYPE=tag",
                environment_details[f"environment {name}"],
            )

    def test_missing_protection_environments_and_secrets_are_actionable(self) -> None:
        environment_secrets = configured_environment_secrets()
        environment_secrets["release"] = set()
        findings = self.audit(
            rulesets=[],
            environments={"environments": []},
            environment_secrets=environment_secrets,
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

        findings = self.audit(immutable_releases=None)
        errors = {finding.subject for finding in findings if finding.level == "error"}
        self.assertIn("immutable releases", errors)

    def test_disabled_or_incomplete_rulesets_do_not_count_as_protection(self) -> None:
        findings = self.audit(
            rulesets=[
                {
                    "target": "branch",
                    "enforcement": "disabled",
                    "bypass_actors": [],
                    "conditions": {"ref_name": {"include": ["~DEFAULT_BRANCH"]}},
                    "rules": [{"type": "pull_request"}],
                },
                {
                    "target": "tag",
                    "enforcement": "active",
                    "bypass_actors": [
                        {
                            "actor_id": 1,
                            "actor_type": "User",
                            "bypass_mode": "always",
                        }
                    ],
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

    def test_main_and_tag_immutability_rulesets_require_no_bypass(self) -> None:
        cases: tuple[tuple[str, str, object], ...] = (
            ("main omitted", "main protection", None),
            (
                "main actor",
                "main protection",
                [
                    {
                        "actor_id": 1,
                        "actor_type": "User",
                        "bypass_mode": "always",
                    }
                ],
            ),
            ("tag immutability omitted", "release tags", None),
            (
                "tag immutability actor",
                "release tags",
                [
                    {
                        "actor_id": 1,
                        "actor_type": "User",
                        "bypass_mode": "exempt",
                    }
                ],
            ),
            ("tag immutability malformed", "release tags", {}),
        )
        for label, subject, bypass_actors in cases:
            with self.subTest(label=label):
                rulesets = configured_rulesets()
                ruleset = rulesets[0 if subject == "main protection" else 2]
                if bypass_actors is None:
                    ruleset.pop("bypass_actors")
                else:
                    ruleset["bypass_actors"] = bypass_actors
                findings = self.audit(rulesets=rulesets)
                errors = {
                    finding.subject for finding in findings if finding.level == "error"
                }
                self.assertIn(subject, errors)

    def test_tag_creation_requires_one_creation_only_controlled_allowlist(
        self,
    ) -> None:
        cases: tuple[str, ...] = (
            "missing bypass actors",
            "empty bypass actors",
            "wrong bypass mode",
            "malformed actor",
            "combined with immutability",
            "additional creation rule",
        )
        for label in cases:
            with self.subTest(label=label):
                rulesets = configured_rulesets()
                creator_rule = rulesets[1]
                if label == "missing bypass actors":
                    creator_rule.pop("bypass_actors")
                elif label == "empty bypass actors":
                    creator_rule["bypass_actors"] = []
                elif label == "wrong bypass mode":
                    creator_rule["bypass_actors"] = [
                        {
                            "actor_id": 1,
                            "actor_type": "User",
                            "bypass_mode": "pull_request",
                        }
                    ]
                elif label == "malformed actor":
                    creator_rule["bypass_actors"] = [
                        {
                            "actor_id": True,
                            "actor_type": "User",
                            "bypass_mode": "always",
                        }
                    ]
                elif label == "combined with immutability":
                    creator_rule["rules"] = [
                        {"type": "creation"},
                        {"type": "update"},
                        {"type": "deletion"},
                    ]
                else:
                    rulesets.append(
                        {
                            "target": "tag",
                            "enforcement": "active",
                            "bypass_actors": [],
                            "conditions": {"ref_name": {"include": ["refs/tags/v*"]}},
                            "rules": [{"type": "creation"}],
                        }
                    )
                findings = self.audit(rulesets=rulesets)
                errors = {
                    finding.subject for finding in findings if finding.level == "error"
                }
                self.assertIn("release tags", errors)

    def test_legacy_main_protection_must_require_a_pull_request(self) -> None:
        rulesets = configured_rulesets()[1:]
        findings = self.audit(
            rulesets=rulesets,
            main_legacy_protection={
                "required_pull_request_reviews": {"required_approving_review_count": 1},
                "required_status_checks": legacy_required_ci_checks(),
            },
        )
        self.assertNotIn("error", [finding.level for finding in findings])

        findings = self.audit(
            rulesets=rulesets,
            main_legacy_protection={"required_status_checks": {"strict": True}},
        )
        errors = {finding.subject for finding in findings if finding.level == "error"}
        self.assertIn("main protection", errors)

    def test_main_ruleset_requires_strict_github_actions_ci_checks(self) -> None:
        cases: tuple[str, ...] = (
            "missing status rule",
            "loose status rule",
            "wrong status source",
            "missing required context",
            "missing force-push protection",
        )
        for label in cases:
            with self.subTest(label=label):
                rulesets = configured_rulesets()
                main_rule = rulesets[0]
                rules = main_rule["rules"]
                assert isinstance(rules, list)
                status_rule = next(
                    rule
                    for rule in rules
                    if isinstance(rule, dict)
                    and rule.get("type") == "required_status_checks"
                )
                assert isinstance(status_rule, dict)
                parameters = status_rule["parameters"]
                assert isinstance(parameters, dict)
                checks = parameters["required_status_checks"]
                assert isinstance(checks, list)
                if label == "missing status rule":
                    main_rule["rules"] = [
                        rule for rule in rules if rule is not status_rule
                    ]
                elif label == "loose status rule":
                    parameters["strict_required_status_checks_policy"] = False
                elif label == "wrong status source":
                    checks[0] = {
                        "context": MAIN_REQUIRED_CHECK_CONTEXTS[0],
                        "integration_id": 1,
                    }
                elif label == "missing required context":
                    parameters["required_status_checks"] = checks[1:]
                else:
                    main_rule["rules"] = [
                        rule
                        for rule in rules
                        if not isinstance(rule, dict)
                        or rule.get("type") != "non_fast_forward"
                    ]
                findings = self.audit(rulesets=rulesets)
                errors = {
                    finding.subject for finding in findings if finding.level == "error"
                }
                self.assertIn("main protection", errors)

    def test_tag_triggered_environments_require_v_tag_policy_even_with_reviewers(
        self,
    ) -> None:
        for index, name in enumerate(
            ("release", "release-qualification", "stable-release")
        ):
            with self.subTest(environment=name):
                environments = configured_environments()
                environment = environments["environments"][index]
                assert isinstance(environment, dict)
                environment["deployment_branch_policy"] = {
                    "protected_branches": True,
                    "custom_branch_policies": False,
                }
                findings = self.audit(environments=environments)
                errors = {
                    finding.subject for finding in findings if finding.level == "error"
                }
                self.assertIn(f"environment {name}", errors)

    def test_custom_tag_policies_require_a_complete_exact_v_pattern_listing(
        self,
    ) -> None:
        environments = configured_environments()
        environments["environments"][0] = {
            "name": "release",
            "deployment_branch_policy": {
                "protected_branches": False,
                "custom_branch_policies": True,
            },
        }
        cases: tuple[tuple[str, object, bool], ...] = (
            ("missing", None, False),
            (
                "incomplete listing",
                {
                    "branch_policies": [
                        {"id": 1, "node_id": "policy-release", "name": "v*"}
                    ]
                },
                False,
            ),
            (
                "missing policy identity",
                {
                    "total_count": 1,
                    "branch_policies": [{"name": "v*"}],
                },
                False,
            ),
            (
                "wrong pattern",
                {
                    "total_count": 1,
                    "branch_policies": [
                        {
                            "id": 1,
                            "node_id": "policy-release",
                            "name": "release/*",
                        }
                    ],
                },
                False,
            ),
            (
                "exact v pattern without target type",
                {
                    "total_count": 1,
                    "branch_policies": [
                        {"id": 1, "node_id": "policy-release", "name": "v*"}
                    ],
                },
                True,
            ),
        )
        for label, policy_response, passes in cases:
            with self.subTest(label=label):
                policies = configured_deployment_branch_policies()
                if policy_response is None:
                    policies.pop("release")
                else:
                    policies["release"] = policy_response
                findings = self.audit(
                    environments=environments,
                    deployment_branch_policies=policies,
                )
                errors = {
                    finding.subject for finding in findings if finding.level == "error"
                }
                if passes:
                    self.assertNotIn("environment release", errors)
                else:
                    self.assertIn("environment release", errors)

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

    def test_stable_release_administrator_bypass_fails_closed(self) -> None:
        for label, value in (
            ("enabled", True),
            ("missing", None),
            ("ambiguous", "false"),
        ):
            with self.subTest(label=label):
                environments = configured_environments()
                stable_release = environments["environments"][2]
                assert isinstance(stable_release, dict)
                if label == "missing":
                    stable_release.pop("can_admins_bypass")
                else:
                    stable_release["can_admins_bypass"] = value
                findings = self.audit(environments=environments)
                errors = {
                    finding.subject: finding.detail
                    for finding in findings
                    if finding.level == "error"
                }
                self.assertEqual(
                    errors["environment stable-release"],
                    "set can_admins_bypass to false to disallow administrator bypass",
                )

    def test_workflow_secret_names_are_isolated_to_release_environment(self) -> None:
        cases: tuple[tuple[str, set[str], str | None], ...] = (
            ("repository", {"GURUTERMINAL_UPDATER_PUBLIC_KEY"}, None),
            (
                "release-qualification",
                {"GURUTERMINAL_WINDOWS_CERTIFICATE"},
                "release-qualification",
            ),
            (
                "stable-release",
                {"GURUTERMINAL_UPDATER_PUBLIC_KEY"},
                "stable-release",
            ),
        )
        for scope, leaked_names, environment_name in cases:
            with self.subTest(scope=scope):
                repository_secrets: set[str] = set()
                environment_secrets = configured_environment_secrets()
                if environment_name is None:
                    repository_secrets = leaked_names
                else:
                    environment_secrets[environment_name] = leaked_names
                findings = self.audit(
                    repository_secrets=repository_secrets,
                    environment_secrets=environment_secrets,
                )
                errors = {
                    finding.subject: finding.detail
                    for finding in findings
                    if finding.level == "error"
                }
                self.assertEqual(
                    errors[
                        f"{scope if environment_name is None else 'environment ' + scope} secret isolation"
                    ],
                    "workflow-referenced secret names must exist only in release: "
                    + ", ".join(sorted(leaked_names)),
                )

    def test_incomplete_name_only_secret_listing_fails_closed(self) -> None:
        with self.assertRaises(AuditError):
            repository_secret_names(
                {"total_count": 101, "secrets": [{"name": "GURUTERMINAL_ONE"}]}
            )

    def test_ruleset_detail_loader_rejects_ambiguous_summaries(self) -> None:
        with self.assertRaises(AuditError):
            load_ruleset_details("read-only-gh", REPOSITORY, [{"id": 1}, {"id": 1}])

        with self.assertRaises(AuditError):
            load_ruleset_details("read-only-gh", REPOSITORY, [{"id": True}])

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

    def test_remote_audit_reads_name_only_secret_and_tag_policy_endpoints(
        self,
    ) -> None:
        environments = configured_environments()
        requests: list[str] = []
        rulesets = [
            {"id": index + 1, **ruleset}
            for index, ruleset in enumerate(configured_rulesets())
        ]
        endpoints: dict[str, object | None] = {
            f"/repos/{REPOSITORY}": {
                "full_name": REPOSITORY,
                "visibility": "public",
                "private": False,
                "default_branch": "main",
            },
            f"/repos/{REPOSITORY}/rulesets?per_page=100&includes_parents=false": [
                {"id": index + 1} for index in range(len(rulesets))
            ],
            **{
                f"/repos/{REPOSITORY}/rulesets/{index + 1}": ruleset
                for index, ruleset in enumerate(rulesets)
            },
            f"/repos/{REPOSITORY}/branches/main/protection": None,
            f"/repos/{REPOSITORY}/immutable-releases": {
                "enabled": True,
                "enforced_by_owner": False,
            },
            f"/repos/{REPOSITORY}/environments?per_page=100": environments,
            f"/repos/{REPOSITORY}/actions/secrets?per_page=100": secret_response(set()),
            f"/repos/{REPOSITORY}/environments/release/secrets?per_page=100": secret_response(
                {"GURUTERMINAL_ONE"}
            ),
            f"/repos/{REPOSITORY}/environments/release-qualification/secrets?per_page=100": secret_response(
                set()
            ),
            f"/repos/{REPOSITORY}/environments/stable-release/secrets?per_page=100": secret_response(
                set()
            ),
            f"/repos/{REPOSITORY}/environments/release/deployment-branch-policies?per_page=100": {
                "total_count": 1,
                "branch_policies": [
                    {"id": 1, "node_id": "policy-release", "name": "v*"}
                ],
            },
            f"/repos/{REPOSITORY}/environments/release-qualification/deployment-branch-policies?per_page=100": {
                "total_count": 1,
                "branch_policies": [
                    {
                        "id": 2,
                        "node_id": "policy-release-qualification",
                        "name": "v*",
                    }
                ],
            },
            f"/repos/{REPOSITORY}/environments/stable-release/deployment-branch-policies?per_page=100": {
                "total_count": 1,
                "branch_policies": [
                    {"id": 3, "node_id": "policy-stable-release", "name": "v*"}
                ],
            },
        }

        def fake_gh_api(
            gh: str, endpoint: str, *, allow_not_found: bool = False
        ) -> object | None:
            self.assertEqual(gh, "read-only-gh")
            self.assertFalse(allow_not_found and endpoint not in endpoints)
            requests.append(endpoint)
            return endpoints[endpoint]

        with tempfile.TemporaryDirectory() as temporary_directory:
            workflow = Path(temporary_directory) / "release.yml"
            workflow.write_text(
                "env:\n  TOKEN: ${{ secrets.GURUTERMINAL_ONE }}\n",
                encoding="utf-8",
            )
            with mock.patch.dict(remote_audit.__globals__, {"gh_api": fake_gh_api}):
                findings = remote_audit(REPOSITORY, workflow, "read-only-gh")

        self.assertNotIn("error", [finding.level for finding in findings])
        self.assertEqual(
            set(requests)
            & {
                f"/repos/{REPOSITORY}/actions/secrets?per_page=100",
                f"/repos/{REPOSITORY}/rulesets?per_page=100&includes_parents=false",
                f"/repos/{REPOSITORY}/rulesets/1",
                f"/repos/{REPOSITORY}/rulesets/2",
                f"/repos/{REPOSITORY}/rulesets/3",
                f"/repos/{REPOSITORY}/environments/release/secrets?per_page=100",
                f"/repos/{REPOSITORY}/environments/release-qualification/secrets?per_page=100",
                f"/repos/{REPOSITORY}/environments/stable-release/secrets?per_page=100",
                f"/repos/{REPOSITORY}/environments/release/deployment-branch-policies?per_page=100",
                f"/repos/{REPOSITORY}/environments/release-qualification/deployment-branch-policies?per_page=100",
                f"/repos/{REPOSITORY}/environments/stable-release/deployment-branch-policies?per_page=100",
            },
            {
                f"/repos/{REPOSITORY}/actions/secrets?per_page=100",
                f"/repos/{REPOSITORY}/rulesets?per_page=100&includes_parents=false",
                f"/repos/{REPOSITORY}/rulesets/1",
                f"/repos/{REPOSITORY}/rulesets/2",
                f"/repos/{REPOSITORY}/rulesets/3",
                f"/repos/{REPOSITORY}/environments/release/secrets?per_page=100",
                f"/repos/{REPOSITORY}/environments/release-qualification/secrets?per_page=100",
                f"/repos/{REPOSITORY}/environments/stable-release/secrets?per_page=100",
                f"/repos/{REPOSITORY}/environments/release/deployment-branch-policies?per_page=100",
                f"/repos/{REPOSITORY}/environments/release-qualification/deployment-branch-policies?per_page=100",
                f"/repos/{REPOSITORY}/environments/stable-release/deployment-branch-policies?per_page=100",
            },
        )


if __name__ == "__main__":
    unittest.main()
