#!/usr/bin/env python3
"""Read-only release-setup audit for the public GitHub repository.

The audit intentionally reads only repository metadata, rules, environments,
and repository/environment-secret *names*. It never creates GitHub state or
reveals a secret value. Keep this outside the normal offline verification gate:
it requires an authenticated ``gh`` client with repository and Environment-read
access.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from collections.abc import Mapping
from dataclasses import dataclass
from fnmatch import fnmatchcase
from pathlib import Path
from urllib.parse import quote


OFFICIAL_REPOSITORY = "monarchjuno/guruterminal"
EXPECTED_DEFAULT_BRANCH = "main"
REQUIRED_ENVIRONMENTS = ("release", "release-qualification", "stable-release")
TAG_TRIGGERED_ENVIRONMENTS = frozenset(REQUIRED_ENVIRONMENTS)
REPOSITORY_PATTERN = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+")
SECRET_REFERENCE = re.compile(r"\bsecrets\.([A-Z][A-Z0-9_]*)\b")
GITHUB_ACTIONS_INTEGRATION_ID = 15368
MAIN_REQUIRED_STATUS_CHECK_CONTEXTS = frozenset(
    (
        "Source and product contracts",
        "Native macOS interaction",
        "Package smoke (aarch64-apple-darwin)",
        "Package smoke (x86_64-pc-windows-msvc)",
    )
)
MAIN_BRANCH_RULE_TYPES = frozenset(
    ("pull_request", "required_status_checks", "non_fast_forward")
)
RELEASE_TAG_IMMUTABILITY_RULE_TYPES = frozenset(("update", "deletion"))
RELEASE_TAG_CREATION_RULE_TYPES = frozenset(("creation",))
RELEASE_TAG_CREATOR_ACTOR_TYPES = frozenset(
    ("User", "Team", "Integration", "RepositoryRole", "OrganizationAdmin")
)


class AuditError(RuntimeError):
    """Raised when remote state cannot be safely interpreted."""


@dataclass(frozen=True)
class Finding:
    level: str
    subject: str
    detail: str


def require_object(value: object, label: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise AuditError(f"{label} must be a JSON object")
    return value


def string_items(value: object, label: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        raise AuditError(f"{label} must be a JSON string array")
    return tuple(value)


def required_release_secret_names(workflow: Path) -> set[str]:
    try:
        text = workflow.read_text(encoding="utf-8")
    except OSError as error:
        raise AuditError(f"cannot read release workflow {workflow}: {error}") from error
    names = {match.group(1) for match in SECRET_REFERENCE.finditer(text)}
    if not names:
        raise AuditError(f"release workflow {workflow} does not reference any secrets")
    return names


def matches_ref_pattern(pattern: str, ref: str, default_ref: str) -> bool:
    if pattern == "~DEFAULT_BRANCH":
        pattern = default_ref
    elif pattern == "~ALL":
        pattern = "*"
    return fnmatchcase(ref, pattern)


def rule_types(value: object) -> set[str] | None:
    if not isinstance(value, list):
        return None
    types: set[str] = set()
    for rule in value:
        if not isinstance(rule, dict):
            return None
        rule_type = rule.get("type")
        if not isinstance(rule_type, str) or not rule_type:
            return None
        types.add(rule_type)
    return types


def status_checks_are_strict_and_github_actions_owned(
    parameters: object,
) -> bool:
    if not isinstance(parameters, dict):
        return False
    if (
        parameters.get("do_not_enforce_on_create") is not False
        or parameters.get("strict_required_status_checks_policy") is not True
    ):
        return False
    checks = parameters.get("required_status_checks")
    if not isinstance(checks, list):
        return False
    contexts: set[str] = set()
    for check in checks:
        if not isinstance(check, dict):
            return False
        context = check.get("context")
        if not isinstance(context, str) or not context:
            return False
        if check.get("integration_id") != GITHUB_ACTIONS_INTEGRATION_ID:
            return False
        contexts.add(context)
    return MAIN_REQUIRED_STATUS_CHECK_CONTEXTS <= contexts


def ruleset_requires_strict_github_actions_ci(ruleset: dict[str, object]) -> bool:
    rules = ruleset.get("rules")
    if not isinstance(rules, list):
        return False
    status_rules = [
        rule
        for rule in rules
        if isinstance(rule, dict) and rule.get("type") == "required_status_checks"
    ]
    return len(status_rules) == 1 and status_checks_are_strict_and_github_actions_owned(
        status_rules[0].get("parameters")
    )


def legacy_main_requires_strict_github_actions_ci(
    protection: dict[str, object],
) -> bool:
    status_checks = protection.get("required_status_checks")
    if not isinstance(status_checks, dict) or status_checks.get("strict") is not True:
        return False
    checks = status_checks.get("checks")
    if not isinstance(checks, list):
        return False
    contexts: set[str] = set()
    for check in checks:
        if not isinstance(check, dict):
            return False
        context = check.get("context")
        if not isinstance(context, str) or not context:
            return False
        if check.get("app_id") != GITHUB_ACTIONS_INTEGRATION_ID:
            return False
        contexts.add(context)
    return MAIN_REQUIRED_STATUS_CHECK_CONTEXTS <= contexts


def ruleset_has_no_bypass_actors(ruleset: dict[str, object]) -> bool:
    """Require an explicit, empty bypass list before trusting a ruleset.

    The REST response can omit ``bypass_actors`` when the caller cannot see
    it. Treating that omission as an empty list would turn limited read access
    into a false protection pass. GitHub's documented bypass modes all grant
    an actor a way around a rule, so no nonempty list is safe for this gate.
    """

    return "bypass_actors" in ruleset and ruleset.get("bypass_actors") == []


def ruleset_has_controlled_creation_bypass(ruleset: dict[str, object]) -> bool:
    """Require an explicit, well-formed creator allowlist for a creation-only rule.

    A no-bypass ``creation`` rule would prevent every release tag from being
    created. The matching update/deletion rule is separately no-bypass, so a
    narrowly scoped creator allowlist here cannot make an existing tag mutable.
    """

    actors = ruleset.get("bypass_actors")
    if not isinstance(actors, list) or not actors:
        return False
    for actor in actors:
        if not isinstance(actor, dict):
            return False
        actor_id = actor.get("actor_id")
        if (
            not isinstance(actor_id, int)
            or isinstance(actor_id, bool)
            or actor_id < 1
            or actor.get("actor_type") not in RELEASE_TAG_CREATOR_ACTOR_TYPES
            or actor.get("bypass_mode") != "always"
        ):
            return False
    return True


def active_ruleset_applies_to_ref(
    ruleset: dict[str, object],
    *,
    target: str,
    ref: str,
    default_ref: str,
) -> bool:
    if ruleset.get("target") != target or ruleset.get("enforcement") != "active":
        return False
    conditions = ruleset.get("conditions")
    if not isinstance(conditions, dict):
        return False
    ref_name = conditions.get("ref_name")
    if not isinstance(ref_name, dict):
        return False
    try:
        includes = string_items(ref_name.get("include"), "ruleset ref include")
        excludes = string_items(ref_name.get("exclude", []), "ruleset ref exclude")
    except AuditError:
        return False
    return any(
        matches_ref_pattern(pattern, ref, default_ref) for pattern in includes
    ) and not any(
        matches_ref_pattern(pattern, ref, default_ref) for pattern in excludes
    )


def active_ruleset_protects(
    ruleset: object,
    *,
    target: str,
    ref: str,
    default_ref: str,
    required_rule_types: frozenset[str],
    forbidden_rule_types: frozenset[str] = frozenset(),
) -> bool:
    value = require_object(ruleset, "repository ruleset")
    if not active_ruleset_applies_to_ref(
        value,
        target=target,
        ref=ref,
        default_ref=default_ref,
    ) or not ruleset_has_no_bypass_actors(value):
        return False
    types = rule_types(value.get("rules"))
    if types is None or not required_rule_types <= types:
        return False
    return not types.intersection(forbidden_rule_types)


def any_ruleset_protects(
    rulesets: object,
    *,
    target: str,
    ref: str,
    default_ref: str,
    required_rule_types: frozenset[str],
    forbidden_rule_types: frozenset[str] = frozenset(),
) -> bool:
    if not isinstance(rulesets, list):
        raise AuditError("repository rulesets response must be a JSON array")
    return any(
        active_ruleset_protects(
            ruleset,
            target=target,
            ref=ref,
            default_ref=default_ref,
            required_rule_types=required_rule_types,
            forbidden_rule_types=forbidden_rule_types,
        )
        for ruleset in rulesets
    )


def any_ruleset_protects_main_with_ci(
    rulesets: object,
    *,
    ref: str,
    default_ref: str,
) -> bool:
    if not isinstance(rulesets, list):
        raise AuditError("repository rulesets response must be a JSON array")
    for ruleset in rulesets:
        if not active_ruleset_protects(
            ruleset,
            target="branch",
            ref=ref,
            default_ref=default_ref,
            required_rule_types=MAIN_BRANCH_RULE_TYPES,
        ):
            continue
        value = require_object(ruleset, "repository ruleset")
        if ruleset_requires_strict_github_actions_ci(value):
            return True
    return False


def active_ruleset_allows_controlled_tag_creation(
    ruleset: object,
    *,
    ref: str,
    default_ref: str,
) -> bool:
    value = require_object(ruleset, "repository ruleset")
    if not active_ruleset_applies_to_ref(
        value,
        target="tag",
        ref=ref,
        default_ref=default_ref,
    ) or not ruleset_has_controlled_creation_bypass(value):
        return False
    return rule_types(value.get("rules")) == RELEASE_TAG_CREATION_RULE_TYPES


def tag_creation_route_is_controlled(
    rulesets: object,
    *,
    ref: str,
    default_ref: str,
) -> bool:
    if not isinstance(rulesets, list):
        raise AuditError("repository rulesets response must be a JSON array")
    matching_creation_rulesets: list[object] = []
    for ruleset in rulesets:
        value = require_object(ruleset, "repository ruleset")
        if not active_ruleset_applies_to_ref(
            value,
            target="tag",
            ref=ref,
            default_ref=default_ref,
        ):
            continue
        types = rule_types(value.get("rules"))
        if types is None:
            return False
        if "creation" in types:
            matching_creation_rulesets.append(ruleset)
    return len(
        matching_creation_rulesets
    ) == 1 and active_ruleset_allows_controlled_tag_creation(
        matching_creation_rulesets[0],
        ref=ref,
        default_ref=default_ref,
    )


def environments_by_name(value: object) -> dict[str, dict[str, object]]:
    payload = require_object(value, "environments response")
    environments = payload.get("environments")
    if not isinstance(environments, list):
        raise AuditError("environments response must contain an environments array")
    result: dict[str, dict[str, object]] = {}
    for environment in environments:
        item = require_object(environment, "environment")
        name = item.get("name")
        if not isinstance(name, str) or not name:
            raise AuditError("environment name must be a nonempty string")
        result[name] = item
    return result


def environment_uses_custom_branch_policies(environment: dict[str, object]) -> bool:
    branch_policy = environment.get("deployment_branch_policy")
    return (
        isinstance(branch_policy, dict)
        and branch_policy.get("custom_branch_policies") is True
    )


def deployment_policy_lists_v_ref_pattern(value: object | None) -> bool:
    """Return whether a complete official policy list contains the exact ``v*`` name.

    GitHub's read-only list/get deployment-policy responses expose ``id``,
    ``node_id``, and ``name``, but not the policy target type. The audit can
    therefore prove only the exact policy name. Tag execution remains enforced
    at runtime by the tag-triggered release workflow and the qualification/
    promotion workflows' ``GITHUB_REF_TYPE=tag`` guards.
    """

    if not isinstance(value, dict):
        return False
    policies = value.get("branch_policies")
    if not isinstance(policies, list):
        return False
    total_count = value.get("total_count")
    if (
        not isinstance(total_count, int)
        or isinstance(total_count, bool)
        or total_count != len(policies)
    ):
        return False
    lists_v_ref_pattern = False
    for policy in policies:
        if not isinstance(policy, dict):
            return False
        policy_id = policy.get("id")
        node_id = policy.get("node_id")
        name = policy.get("name")
        if (
            not isinstance(policy_id, int)
            or isinstance(policy_id, bool)
            or policy_id < 1
            or not isinstance(node_id, str)
            or not node_id
            or not isinstance(name, str)
            or not name
        ):
            return False
        if name == "v*":
            lists_v_ref_pattern = True
    return lists_v_ref_pattern


def environment_is_protected(
    environment: dict[str, object],
    *,
    requires_v_tag_policy: bool,
    has_v_ref_policy: bool,
) -> bool:
    if requires_v_tag_policy and (
        not environment_uses_custom_branch_policies(environment) or not has_v_ref_policy
    ):
        return False
    protection_rules = environment.get("protection_rules")
    if isinstance(protection_rules, list) and protection_rules:
        return True
    branch_policy = environment.get("deployment_branch_policy")
    if not isinstance(branch_policy, dict):
        return False
    if branch_policy.get("protected_branches") is True:
        return True
    if branch_policy.get("custom_branch_policies") is True:
        return has_v_ref_policy if requires_v_tag_policy else True
    return False


def environment_requires_independent_reviewer(environment: dict[str, object]) -> bool:
    """Return whether an environment needs a reviewer other than the initiator."""

    protection_rules = environment.get("protection_rules")
    if not isinstance(protection_rules, list):
        return False
    for rule in protection_rules:
        if not isinstance(rule, dict) or rule.get("type") != "required_reviewers":
            continue
        reviewers = rule.get("reviewers")
        if rule.get("prevent_self_review") is True and isinstance(reviewers, list):
            return bool(reviewers)
    return False


def environment_disallows_administrator_bypass(environment: dict[str, object]) -> bool:
    """Return whether GitHub explicitly prevents administrators from bypassing it."""

    # Treat a missing or differently typed value as unsafe.  The stable release
    # gate must not rely on a default that could change or be omitted by an API.
    return environment.get("can_admins_bypass") is False


def immutable_releases_are_enabled(value: object | None) -> bool:
    if value is None:
        return False
    release_settings = require_object(value, "immutable releases response")
    return (
        release_settings.get("enabled") is True
        or release_settings.get("enforced_by_owner") is True
    )


def legacy_main_has_pull_request_protection(value: object | None) -> bool:
    if value is None:
        return False
    protection = require_object(value, "main branch protection response")
    return isinstance(
        protection.get("required_pull_request_reviews"), dict
    ) and legacy_main_requires_strict_github_actions_ci(protection)


def secret_names(value: object, *, response_label: str, item_label: str) -> set[str]:
    payload = require_object(value, response_label)
    secrets = payload.get("secrets")
    if not isinstance(secrets, list):
        raise AuditError(f"{response_label} must contain a secrets array")
    total_count = payload.get("total_count")
    if (
        not isinstance(total_count, int)
        or isinstance(total_count, bool)
        or total_count != len(secrets)
    ):
        raise AuditError(f"{response_label} must return a complete secrets array")
    names: set[str] = set()
    for secret in secrets:
        item = require_object(secret, item_label)
        name = item.get("name")
        if not isinstance(name, str) or not name:
            raise AuditError(f"{item_label} name must be a nonempty string")
        names.add(name)
    return names


def environment_secret_names(value: object) -> set[str]:
    return secret_names(
        value,
        response_label="environment secrets response",
        item_label="environment secret",
    )


def repository_secret_names(value: object) -> set[str]:
    return secret_names(
        value,
        response_label="repository secrets response",
        item_label="repository secret",
    )


def secret_names_for_environment(
    environment_secrets: Mapping[str, set[str]], environment_name: str
) -> set[str]:
    names = environment_secrets.get(environment_name, set())
    if not isinstance(names, set) or not all(
        isinstance(name, str) and name for name in names
    ):
        raise AuditError(f"secret names for environment {environment_name} are invalid")
    return names


def audit_release_setup(
    *,
    repository: str,
    repository_secrets: set[str],
    environment_secrets: Mapping[str, set[str]],
    deployment_branch_policies: Mapping[str, object],
    required_secrets: set[str],
    repository_data: object,
    rulesets: object,
    main_legacy_protection: object | None,
    immutable_releases: object | None,
    environments: object,
) -> list[Finding]:
    """Evaluate supplied public metadata without performing network I/O."""

    findings: list[Finding] = []

    def add(level: str, subject: str, detail: str) -> None:
        findings.append(Finding(level, subject, detail))

    metadata = require_object(repository_data, "repository response")
    default_ref = f"refs/heads/{EXPECTED_DEFAULT_BRANCH}"
    public_repository = (
        metadata.get("full_name") == repository
        and metadata.get("visibility") == "public"
        and metadata.get("private") is False
    )
    if public_repository:
        add(
            "pass", "repository visibility", "repository is public and identity matches"
        )
    else:
        add(
            "error",
            "repository visibility",
            "repository must be the public official repository",
        )

    if metadata.get("default_branch") == EXPECTED_DEFAULT_BRANCH:
        add("pass", "default branch", "default branch is main")
    else:
        add("error", "default branch", "default branch must be main")

    if immutable_releases_are_enabled(immutable_releases):
        add(
            "pass",
            "immutable releases",
            "release assets cannot be changed after publication",
        )
    else:
        add("error", "immutable releases", "enable immutable GitHub Releases")

    main_protected = legacy_main_has_pull_request_protection(
        main_legacy_protection
    ) or any_ruleset_protects_main_with_ci(
        rulesets,
        ref=default_ref,
        default_ref=default_ref,
    )
    if main_protected:
        add(
            "pass",
            "main protection",
            "a no-bypass pull-request rule and strict GitHub Actions CI checks apply",
        )
    else:
        add(
            "error",
            "main protection",
            "require pull requests and strict GitHub Actions CI checks for main "
            "with a branch protection or active ruleset",
        )

    release_tag_ref = "refs/tags/v0.0.1"
    tag_immutable = any_ruleset_protects(
        rulesets,
        target="tag",
        ref=release_tag_ref,
        default_ref=default_ref,
        required_rule_types=RELEASE_TAG_IMMUTABILITY_RULE_TYPES,
        forbidden_rule_types=RELEASE_TAG_CREATION_RULE_TYPES,
    )
    tag_creation_controlled = tag_creation_route_is_controlled(
        rulesets,
        ref=release_tag_ref,
        default_ref=default_ref,
    )
    if tag_immutable and tag_creation_controlled:
        add(
            "pass",
            "release tags",
            "a creation-only creator allowlist and a separate no-bypass v* "
            "update/deletion ruleset keep release tags creatable and immutable",
        )
    else:
        add(
            "error",
            "release tags",
            "configure a creation-only v* ruleset with an explicit creator allowlist "
            "and a separate no-bypass v* update/deletion ruleset",
        )

    configured_environments = environments_by_name(environments)
    for name in REQUIRED_ENVIRONMENTS:
        environment = configured_environments.get(name)
        if environment is None:
            add(
                "error",
                f"environment {name}",
                "required release environment is missing",
            )
        elif name == "stable-release" and not environment_requires_independent_reviewer(
            environment
        ):
            add(
                "error",
                f"environment {name}",
                "configure a required reviewer and prevent self-review",
            )
        elif (
            name == "stable-release"
            and not environment_disallows_administrator_bypass(environment)
        ):
            add(
                "error",
                f"environment {name}",
                "set can_admins_bypass to false to disallow administrator bypass",
            )
        elif environment_is_protected(
            environment,
            requires_v_tag_policy=name in TAG_TRIGGERED_ENVIRONMENTS,
            has_v_ref_policy=deployment_policy_lists_v_ref_pattern(
                deployment_branch_policies.get(name)
            ),
        ):
            add(
                "pass",
                f"environment {name}",
                "deployment protection and an exact v* custom policy are configured; "
                "the policy-list API has no target type, so tag triggers and "
                "GITHUB_REF_TYPE=tag guards enforce tag execution at runtime",
            )
        else:
            if name in TAG_TRIGGERED_ENVIRONMENTS:
                detail = (
                    "configure the custom deployment policy with an exact v* pattern; "
                    "the policy-list API has no target type, so tag triggers and "
                    "GITHUB_REF_TYPE=tag guards enforce tag execution at runtime"
                )
            else:
                detail = "configure a reviewer, wait timer, or deployment branch policy"
            add(
                "error",
                f"environment {name}",
                detail,
            )

    release_secrets = secret_names_for_environment(environment_secrets, "release")
    missing_secrets = sorted(required_secrets - release_secrets)
    if missing_secrets:
        add(
            "error",
            "release environment secrets",
            "missing workflow-referenced secret names: " + ", ".join(missing_secrets),
        )
    else:
        add(
            "pass",
            "release environment secrets",
            "all release workflow secret names are present",
        )

    secret_scopes: tuple[tuple[str, set[str]], ...] = (
        ("repository", repository_secrets),
        (
            "environment release-qualification",
            secret_names_for_environment(environment_secrets, "release-qualification"),
        ),
        (
            "environment stable-release",
            secret_names_for_environment(environment_secrets, "stable-release"),
        ),
    )
    for scope, names in secret_scopes:
        if not isinstance(names, set) or not all(
            isinstance(name, str) and name for name in names
        ):
            raise AuditError(f"secret names for {scope} are invalid")
        leaked_names = sorted(required_secrets & names)
        subject = f"{scope} secret isolation"
        if leaked_names:
            add(
                "error",
                subject,
                "workflow-referenced secret names must exist only in release: "
                + ", ".join(leaked_names),
            )
        else:
            add(
                "pass",
                subject,
                "no release workflow secret names are present",
            )
    return findings


def gh_api(gh: str, endpoint: str, *, allow_not_found: bool = False) -> object | None:
    completed = subprocess.run(
        [gh, "api", endpoint],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        message = completed.stderr.strip() or completed.stdout.strip()
        if allow_not_found and "HTTP 404" in message:
            return None
        raise AuditError(f"GitHub API request failed for {endpoint}: {message}")
    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise AuditError(f"GitHub API response for {endpoint} was not JSON") from error


def remote_audit(repository: str, workflow: Path, gh: str) -> list[Finding]:
    if REPOSITORY_PATTERN.fullmatch(repository) is None:
        raise AuditError("--repository must be an owner/name pair")
    required_secrets = required_release_secret_names(workflow)
    repository_data = gh_api(gh, f"/repos/{repository}")
    rulesets = gh_api(gh, f"/repos/{repository}/rulesets?per_page=100")
    main_protection = gh_api(
        gh,
        f"/repos/{repository}/branches/{EXPECTED_DEFAULT_BRANCH}/protection",
        allow_not_found=True,
    )
    immutable_releases = gh_api(
        gh,
        f"/repos/{repository}/immutable-releases",
        allow_not_found=True,
    )
    environments = gh_api(gh, f"/repos/{repository}/environments?per_page=100")
    configured_environments = environments_by_name(environments)
    repository_secrets = repository_secret_names(
        gh_api(gh, f"/repos/{repository}/actions/secrets?per_page=100")
    )
    environment_secrets: dict[str, set[str]] = {}
    deployment_branch_policies: dict[str, object] = {}
    for name in REQUIRED_ENVIRONMENTS:
        environment = configured_environments.get(name)
        if environment is None:
            continue
        environment_secrets[name] = environment_secret_names(
            gh_api(
                gh,
                f"/repos/{repository}/environments/{quote(name, safe='')}/secrets?per_page=100",
            )
        )
        if (
            name in TAG_TRIGGERED_ENVIRONMENTS
            and environment_uses_custom_branch_policies(environment)
        ):
            deployment_branch_policies[name] = gh_api(
                gh,
                f"/repos/{repository}/environments/{quote(name, safe='')}/deployment-branch-policies?per_page=100",
            )
    return audit_release_setup(
        repository=repository,
        repository_secrets=repository_secrets,
        environment_secrets=environment_secrets,
        deployment_branch_policies=deployment_branch_policies,
        required_secrets=required_secrets,
        repository_data=repository_data,
        rulesets=rulesets,
        main_legacy_protection=main_protection,
        immutable_releases=immutable_releases,
        environments=environments,
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", default=OFFICIAL_REPOSITORY)
    parser.add_argument(
        "--workflow",
        type=Path,
        default=Path(__file__).resolve().parents[3] / ".github/workflows/release.yml",
    )
    parser.add_argument("--gh", default="gh", help="GitHub CLI executable")
    arguments = parser.parse_args()

    try:
        findings = remote_audit(arguments.repository, arguments.workflow, arguments.gh)
    except AuditError as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 2

    print(f"GitHub release setup audit: {arguments.repository}")
    for finding in findings:
        print(f"{finding.level.upper()}: {finding.subject}: {finding.detail}")
    return 1 if any(finding.level == "error" for finding in findings) else 0


if __name__ == "__main__":
    raise SystemExit(main())
