#!/usr/bin/env python3
"""Create or verify the receipt required to publish a stable draft."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path
from urllib.parse import urlparse


STABLE_TAG = re.compile(r"^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
RC_TAG = re.compile(r"^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)-rc\.([1-9]\d*)$")
REPOSITORY = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
SOURCE_COMMIT = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
MACOS_BUNDLE_VERSION = re.compile(r"^[1-9][0-9]*$")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def stable_version(tag: str) -> tuple[int, int, int]:
    match = STABLE_TAG.fullmatch(tag)
    if match is None:
        raise RuntimeError(f"candidate must be a stable vMAJOR.MINOR.PATCH tag: {tag}")
    return tuple(map(int, match.groups()))


def predecessor_version(tag: str) -> tuple[int, int, int, int, int]:
    stable = STABLE_TAG.fullmatch(tag)
    if stable is not None:
        major, minor, patch = map(int, stable.groups())
        return major, minor, patch, 1, 0
    rc = RC_TAG.fullmatch(tag)
    if rc is not None:
        major, minor, patch, sequence = map(int, rc.groups())
        return major, minor, patch, 0, sequence
    raise RuntimeError(
        f"predecessor must be a stable or canonical RC release tag: {tag}"
    )


def evidence_url(value: str, label: str) -> str:
    parsed = urlparse(value)
    if (
        parsed.scheme != "https"
        or not parsed.netloc
        or parsed.username is not None
        or parsed.password is not None
        or parsed.query
        or parsed.fragment
    ):
        raise RuntimeError(
            f"{label} must be a credential-free HTTPS URL without a query or fragment"
        )
    return value


def positive_integer(value: str, label: str) -> str:
    if not value.isascii() or not value.isdigit() or int(value) < 1:
        raise RuntimeError(f"{label} must be a positive decimal integer")
    return value


def load_metadata(path: Path) -> dict[str, object]:
    if path.is_symlink() or not path.is_file():
        raise RuntimeError("release metadata must be a regular file")
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise RuntimeError("release metadata must be a JSON object")
    return value


def candidate_set_sha256(release_metadata: Path) -> str:
    checksums = release_metadata.with_name("SHA256SUMS")
    if (
        checksums.is_symlink()
        or not checksums.is_file()
        or checksums.stat().st_size == 0
    ):
        raise RuntimeError("SHA256SUMS must be a nonempty regular file")
    return sha256(checksums)


def required_candidate_digest(value: str, label: str, expected: str) -> str:
    if not isinstance(value, str) or SHA256.fullmatch(value) is None:
        raise RuntimeError(f"{label} must be a lowercase SHA-256 digest")
    if value != expected:
        raise RuntimeError(f"{label} does not match the candidate set")
    return value


def validate_identity(
    *,
    candidate_tag: str,
    previous_tag: str,
    repository: str,
    source_commit: str,
    release_id: str,
    metadata: dict[str, object],
) -> str:
    candidate_version = stable_version(candidate_tag)
    previous_version = predecessor_version(previous_tag)
    candidate_order = (*candidate_version, 1, 0)
    if previous_version >= candidate_order:
        raise RuntimeError("the qualified installed release must precede the candidate")
    if not REPOSITORY.fullmatch(repository):
        raise RuntimeError("repository must be an owner/name pair")
    if not SOURCE_COMMIT.fullmatch(source_commit):
        raise RuntimeError("source commit must be a lowercase 40-digit SHA-1")
    positive_integer(release_id, "draft release id")
    if (
        metadata.get("schema_version") != 2
        or metadata.get("tag") != candidate_tag
        or metadata.get("version") != candidate_tag.removeprefix("v")
        or metadata.get("repository") != repository
        or metadata.get("source_commit") != source_commit
        or not isinstance(metadata.get("macos_bundle_version"), str)
        or MACOS_BUNDLE_VERSION.fullmatch(metadata["macos_bundle_version"]) is None
    ):
        raise RuntimeError("release metadata identity does not match qualification")
    return candidate_tag.removeprefix("v")


def write_receipt(arguments: argparse.Namespace) -> int:
    metadata = load_metadata(arguments.release_metadata)
    candidate_digest = candidate_set_sha256(arguments.release_metadata)
    candidate_version = validate_identity(
        candidate_tag=arguments.candidate_tag,
        previous_tag=arguments.previous_tag,
        repository=arguments.repository,
        source_commit=arguments.source_commit,
        release_id=arguments.release_id,
        metadata=metadata,
    )
    run_id = positive_integer(
        arguments.workflow_run_id, "qualification workflow run id"
    )
    receipt = {
        "schema_version": 1,
        "repository": arguments.repository,
        "candidate_tag": arguments.candidate_tag,
        "candidate_version": candidate_version,
        "qualified_predecessor_tag": arguments.previous_tag,
        "source_commit": arguments.source_commit,
        "draft_release_id": arguments.release_id,
        "release_metadata_sha256": sha256(arguments.release_metadata),
        "candidate_set_sha256": candidate_digest,
        "qualification_workflow_run_id": run_id,
        "platforms": {
            "macos-aarch64": {
                "result": "passed",
                "evidence_url": evidence_url(
                    arguments.macos_evidence_url, "macOS evidence URL"
                ),
                "candidate_set_sha256": required_candidate_digest(
                    arguments.macos_candidate_set_sha256,
                    "macOS candidate-set digest",
                    candidate_digest,
                ),
            },
            "windows-x86_64": {
                "result": "passed",
                "evidence_url": evidence_url(
                    arguments.windows_evidence_url, "Windows evidence URL"
                ),
                "candidate_set_sha256": required_candidate_digest(
                    arguments.windows_candidate_set_sha256,
                    "Windows candidate-set digest",
                    candidate_digest,
                ),
            },
        },
    }
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    arguments.output.write_text(
        json.dumps(receipt, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return 0


def verify_receipt(arguments: argparse.Namespace) -> int:
    metadata = load_metadata(arguments.release_metadata)
    receipt = json.loads(arguments.receipt.read_text(encoding="utf-8"))
    if not isinstance(receipt, dict):
        raise RuntimeError("qualification receipt must be a JSON object")
    required = {
        "schema_version",
        "repository",
        "candidate_tag",
        "candidate_version",
        "qualified_predecessor_tag",
        "source_commit",
        "draft_release_id",
        "release_metadata_sha256",
        "candidate_set_sha256",
        "qualification_workflow_run_id",
        "platforms",
    }
    if set(receipt) != required or receipt["schema_version"] != 1:
        raise RuntimeError("qualification receipt schema is not exactly version 1")
    candidate_version = validate_identity(
        candidate_tag=arguments.candidate_tag,
        previous_tag=receipt["qualified_predecessor_tag"],
        repository=arguments.repository,
        source_commit=arguments.source_commit,
        release_id=arguments.release_id,
        metadata=metadata,
    )
    expected_scalars = {
        "repository": arguments.repository,
        "candidate_tag": arguments.candidate_tag,
        "candidate_version": candidate_version,
        "source_commit": arguments.source_commit,
        "draft_release_id": arguments.release_id,
        "candidate_set_sha256": candidate_set_sha256(arguments.release_metadata),
        "release_metadata_sha256": sha256(arguments.release_metadata),
        "qualification_workflow_run_id": positive_integer(
            arguments.workflow_run_id, "qualification workflow run id"
        ),
    }
    for key, expected in expected_scalars.items():
        if receipt[key] != expected:
            raise RuntimeError(f"qualification receipt field does not match: {key}")
    expected_platforms = {"macos-aarch64", "windows-x86_64"}
    platforms = receipt["platforms"]
    if not isinstance(platforms, dict) or set(platforms) != expected_platforms:
        raise RuntimeError("qualification receipt must cover both supported platforms")
    for platform, result in platforms.items():
        if not isinstance(result, dict) or set(result) != {
            "result",
            "evidence_url",
            "candidate_set_sha256",
        }:
            raise RuntimeError(f"qualification result is malformed: {platform}")
        if result["result"] != "passed":
            raise RuntimeError(f"qualification did not pass: {platform}")
        evidence_url(result["evidence_url"], f"{platform} evidence URL")
        required_candidate_digest(
            result["candidate_set_sha256"],
            f"{platform} candidate-set digest",
            receipt["candidate_set_sha256"],
        )
    return 0


def print_digest(arguments: argparse.Namespace) -> int:
    load_metadata(arguments.release_metadata)
    print(candidate_set_sha256(arguments.release_metadata))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    write = subparsers.add_parser("write")
    write.add_argument("--candidate-tag", required=True)
    write.add_argument("--previous-tag", required=True)
    write.add_argument("--repository", required=True)
    write.add_argument("--source-commit", required=True)
    write.add_argument("--release-id", required=True)
    write.add_argument("--release-metadata", required=True, type=Path)
    write.add_argument("--workflow-run-id", required=True)
    write.add_argument("--macos-evidence-url", required=True)
    write.add_argument("--macos-candidate-set-sha256", required=True)
    write.add_argument("--windows-evidence-url", required=True)
    write.add_argument("--windows-candidate-set-sha256", required=True)
    write.add_argument("--output", required=True, type=Path)
    write.set_defaults(handler=write_receipt)

    verify = subparsers.add_parser("verify")
    verify.add_argument("--candidate-tag", required=True)
    verify.add_argument("--repository", required=True)
    verify.add_argument("--source-commit", required=True)
    verify.add_argument("--release-id", required=True)
    verify.add_argument("--release-metadata", required=True, type=Path)
    verify.add_argument("--workflow-run-id", required=True)
    verify.add_argument("--receipt", required=True, type=Path)
    verify.set_defaults(handler=verify_receipt)

    digest = subparsers.add_parser("digest")
    digest.add_argument("--release-metadata", required=True, type=Path)
    digest.set_defaults(handler=print_digest)

    arguments = parser.parse_args()
    return arguments.handler(arguments)


if __name__ == "__main__":
    raise SystemExit(main())
