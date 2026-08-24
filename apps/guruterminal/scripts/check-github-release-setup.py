#!/usr/bin/env python3
"""Read-only release-setup audit for the public GitHub repository.

The audit intentionally reads only repository metadata, rules, environments,
and environment-secret *names*. It never creates GitHub state or reveals a
secret value. Keep this outside the normal offline verification gate: it
requires an authenticated ``gh`` client with repository and Environment-read
access.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import dataclass
from fnmatch import fnmatchcase
from pathlib import Path
from urllib.parse import quote


OFFICIAL_REPOSITORY = "monarchjuno/guruterminal"
EXPECTED_DEFAULT_BRANCH = "main"
REQUIRED_ENVIRONMENTS = ("release", "release-qualification", "stable-release")
REPOSITORY_PATTERN = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+")
SECRET_REFERENCE = re.compile(r"\bsecrets\.([A-Z][A-Z0-9_]*)\b")


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


def active_ruleset_protects(
    ruleset: object,
    *,
    target: str,
    ref: str,
    default_ref: str,
) -> bool:
    value = require_object(ruleset, "repository ruleset")
    if value.get("target") != target or value.get("enforcement") != "active":
        return False
    rules = value.get("rules")
    if not isinstance(rules, list) or not rules:
        return False
    conditions = value.get("conditions")
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


def any_ruleset_protects(
    rulesets: object,
    *,
    target: str,
    ref: str,
    default_ref: str,
) -> bool:
    if not isinstance(rulesets, list):
        raise AuditError("repository rulesets response must be a JSON array")
    return any(
        active_ruleset_protects(
            ruleset,
            target=target,
            ref=ref,
            default_ref=default_ref,
        )
        for ruleset in rulesets
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


def environment_is_protected(environment: dict[str, object]) -> bool:
    protection_rules = environment.get("protection_rules")
    if isinstance(protection_rules, list) and protection_rules:
        return True
    branch_policy = environment.get("deployment_branch_policy")
    return isinstance(branch_policy, dict) and bool(
        branch_policy.get("protected_branches")
        or branch_policy.get("custom_branch_policies")
    )


def environment_secret_names(value: object) -> set[str]:
    payload = require_object(value, "environment secrets response")
    secrets = payload.get("secrets")
    if not isinstance(secrets, list):
        raise AuditError("environment secrets response must contain a secrets array")
    names: set[str] = set()
    for secret in secrets:
        item = require_object(secret, "environment secret")
        name = item.get("name")
        if not isinstance(name, str) or not name:
            raise AuditError("environment secret name must be a nonempty string")
        names.add(name)
    return names


def audit_release_setup(
    *,
    repository: str,
    release_secrets: set[str],
    required_secrets: set[str],
    repository_data: object,
    rulesets: object,
    main_has_legacy_protection: bool,
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

    main_protected = main_has_legacy_protection or any_ruleset_protects(
        rulesets,
        target="branch",
        ref=default_ref,
        default_ref=default_ref,
    )
    if main_protected:
        add("pass", "main protection", "an active branch protection or ruleset applies")
    else:
        add(
            "error",
            "main protection",
            "protect main with a branch protection or active ruleset",
        )

    tag_protected = any_ruleset_protects(
        rulesets,
        target="tag",
        ref="refs/tags/v0.0.1",
        default_ref=default_ref,
    )
    if tag_protected:
        add("pass", "release tags", "an active tag ruleset covers v* tags")
    else:
        add("error", "release tags", "protect v* tags with an active tag ruleset")

    configured_environments = environments_by_name(environments)
    for name in REQUIRED_ENVIRONMENTS:
        environment = configured_environments.get(name)
        if environment is None:
            add(
                "error",
                f"environment {name}",
                "required release environment is missing",
            )
        elif environment_is_protected(environment):
            add("pass", f"environment {name}", "deployment protection is configured")
        else:
            add(
                "error",
                f"environment {name}",
                "configure a reviewer, wait timer, or deployment branch policy",
            )

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
    environments = gh_api(gh, f"/repos/{repository}/environments?per_page=100")
    configured_environments = environments_by_name(environments)
    release_secrets: set[str] = set()
    if "release" in configured_environments:
        release_secrets = environment_secret_names(
            gh_api(
                gh,
                f"/repos/{repository}/environments/{quote('release', safe='')}/secrets?per_page=100",
            )
        )
    return audit_release_setup(
        repository=repository,
        release_secrets=release_secrets,
        required_secrets=required_secrets,
        repository_data=repository_data,
        rulesets=rulesets,
        main_has_legacy_protection=main_protection is not None,
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
