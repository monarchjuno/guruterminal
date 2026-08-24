from __future__ import annotations

import json
import os
import sys
from pathlib import Path

import pytest

from guruterminal_openbb.live_parity import (
    Facet,
    audit_providerless_surface,
    deterministic_summary,
    provider_ids,
    run_live_parity,
    semantic_candidates,
    validate_data_result,
)
from guruterminal_openbb.manifest import load_runtime_manifest
from guruterminal_openbb.runtime_client import RuntimeClient


def _tool(
    name: str,
    description: str,
    *,
    providers: list[str] | None,
    fields: tuple[str, ...] = (),
) -> dict[str, object]:
    properties: dict[str, object] = {field: {"type": "string"} for field in fields}
    if providers is not None:
        properties["provider"] = {"type": "string", "enum": providers}
    return {
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": properties},
    }


def test_semantic_discovery_uses_runtime_description_not_fixed_name() -> None:
    facet = Facet(
        "quote",
        (("quote",), ("stock", "equity")),
        frozenset({"symbol"}),
        lambda _schema, provider: {"provider": provider, "symbol": "AAPL"},
    )
    renamed = _tool(
        "runtime_generated_name",
        "Get the latest quote for a stock.",
        providers=["yfinance"],
        fields=("symbol",),
    )

    assert semantic_candidates([renamed], facet) == [renamed]
    assert provider_ids(renamed) == {"yfinance"}


def test_data_result_requires_canonical_top_level_provider() -> None:
    valid, reason, _warnings = validate_data_result(
        {
            "isError": False,
            "structuredContent": {
                "provider": "yfinance",
                "results": [{"provider": "untrusted-row-value"}],
            },
        },
        "yfinance",
    )
    assert valid is True
    assert reason is None

    valid, reason, _warnings = validate_data_result(
        {
            "isError": False,
            "structuredContent": {
                "results": [{"provider": "yfinance"}],
            },
        },
        "yfinance",
    )
    assert valid is False
    assert "got None" in str(reason)


def test_data_result_can_require_actual_structured_fields() -> None:
    result = {
        "isError": False,
        "structuredContent": {
            "provider": "yfinance",
            "results": [{"symbol": "AAPL", "overall_risk": 1.0}],
        },
    }

    valid, reason, _warnings = validate_data_result(
        result,
        "yfinance",
        frozenset({"overall_risk", "audit_risk"}),
    )
    assert valid is True
    assert reason is None

    valid, reason, _warnings = validate_data_result(
        result,
        "yfinance",
        frozenset({"environmental_score", "social_score"}),
    )
    assert valid is False
    assert "required data fields" in str(reason)


def test_providerless_inventory_flags_unknown_tools() -> None:
    manifest = load_runtime_manifest()
    known_name = manifest["providerless_tool_policy"]["local_tools"][0]
    tools = [
        _tool(known_name, "Local computation", providers=None),
        _tool("future_external_route", "Network data", providers=None),
    ]

    audit = audit_providerless_surface(tools, manifest)

    assert audit["local"] == [known_name]
    assert audit["unknown"] == ["future_external_route"]


def test_deterministic_summary_removes_live_values_but_keeps_failure_evidence() -> None:
    summary = deterministic_summary(
        {
            "started_at": "volatile",
            "completed_at": "volatile",
            "runtime": {
                "runtime_id": "openbb",
                "packages": {"openbb": "4.7.2"},
                "uv_lock_sha256": "digest",
                "executable": "platform-specific",
            },
            "summary": {"passed": 0, "partial": 0, "failed": 1},
            "inventory": {"providerless": {"unknown": []}},
            "capabilities": [
                {
                    "capability": "credentialed_only_probe",
                    "status": "failed",
                    "facets": [
                        {
                            "facet": "environmental_social_scores",
                            "status": "not_discovered",
                            "reason": "credentialed provider only",
                            "elapsed_ms": 123,
                            "arguments": {"symbol": "AAPL"},
                            "candidates": [
                                {
                                    "tool": "generated_name",
                                    "providers": ["credentialed"],
                                }
                            ],
                        }
                    ],
                }
            ],
        }
    )

    rendered = json.dumps(summary, sort_keys=True)
    assert "volatile" not in rendered
    assert "elapsed_ms" not in rendered
    assert "arguments" not in rendered
    assert summary["capabilities"][0]["facets"][0]["candidates"] == [
        {"tool": "generated_name", "providers": ["credentialed"]}
    ]


def test_runtime_providerless_inventory_is_fully_classified() -> None:
    manifest = load_runtime_manifest()
    executable_name = (
        "guruterminal-openbb.exe" if os.name == "nt" else "guruterminal-openbb"
    )
    executable = Path(sys.executable).parent / executable_name
    with RuntimeClient(
        executable,
        manifest,
        enabled_provider_ids=[provider["id"] for provider in manifest["providers"]],
    ) as client:
        _categories, tools = client.discover_all_tools()

    audit = audit_providerless_surface(tools, manifest)
    policy = manifest["providerless_tool_policy"]
    assert set(audit["exposed"]) == set(policy["local_tools"]) | set(
        policy["implicit_provider"]
    )
    assert audit["unknown"] == []


@pytest.mark.live
def test_staged_keyless_openbb_parity() -> None:
    if os.environ.get("GURUTERMINAL_OPENBB_LIVE") != "1":
        pytest.skip("set GURUTERMINAL_OPENBB_LIVE=1 to run network parity")
    configured = os.environ.get("GURUTERMINAL_OPENBB_BUNDLE")
    bundle = (
        Path(configured)
        if configured
        else Path(__file__).resolve().parents[2]
        / "src-tauri"
        / "resources"
        / "pi-runtime"
        / "openbb"
    )

    report = run_live_parity(bundle)
    failures = [
        capability
        for capability in report["capabilities"]
        if capability["status"] != "passed"
    ]
    assert not failures, json.dumps(failures, ensure_ascii=False, indent=2)
