"""Opt-in live parity audit for OpenBB's bundled keyless providers."""

from __future__ import annotations

import argparse
import json
import time
from collections.abc import Callable
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

from guruterminal_openbb.runtime_client import RuntimeClient, resolve_bundle

_ADMIN_TOOLS = {
    "available_categories",
    "available_tools",
    "activate_tools",
    "deactivate_tools",
    "activate_category",
}


@dataclass(frozen=True, slots=True)
class Facet:
    """One semantically discovered part of a legacy capability.

    ``provider_preferences`` is an ordered allowlist when non-empty. This
    prevents a semantically adjacent provider route from satisfying a facet.
    """

    id: str
    term_groups: tuple[tuple[str, ...], ...]
    schema_fields: frozenset[str]
    arguments: Callable[[dict[str, Any], str], dict[str, Any]]
    provider_preferences: tuple[str, ...] = ()
    result_fields_any: frozenset[str] = frozenset()


@dataclass(frozen=True, slots=True)
class Capability:
    """A parity capability that may need more than one OpenBB Tool."""

    id: str
    description: str
    facets: tuple[Facet, ...]


def _arguments(
    **values: Any,
) -> Callable[[dict[str, Any], str], dict[str, Any]]:
    def build(schema: dict[str, Any], provider: str) -> dict[str, Any]:
        properties = schema.get("properties", {})
        rendered = {"provider": provider, **values}
        return {key: value for key, value in rendered.items() if key in properties}

    return build


def _history_arguments(schema: dict[str, Any], provider: str) -> dict[str, Any]:
    end = datetime.now(timezone.utc).date()
    start = end - timedelta(days=14)
    return _arguments(
        symbol="AAPL",
        start_date=start.isoformat(),
        end_date=end.isoformat(),
    )(schema, provider)


def _calendar_arguments(schema: dict[str, Any], provider: str) -> dict[str, Any]:
    start = datetime.now(timezone.utc).date()
    end = start + timedelta(days=14)
    return _arguments(
        start_date=start.isoformat(),
        end_date=end.isoformat(),
    )(schema, provider)


def _actions_arguments(schema: dict[str, Any], provider: str) -> dict[str, Any]:
    end = datetime.now(timezone.utc).date()
    start = end - timedelta(days=400)
    return _arguments(
        symbol="AAPL",
        start_date=start.isoformat(),
        end_date=end.isoformat(),
        interval="1d",
        include_actions=True,
    )(schema, provider)


_CAPABILITIES = (
    Capability(
        "symbol_search",
        "Resolve a company name to a listed symbol.",
        (
            Facet(
                "search",
                (("search", "lookup"), ("stock symbol", "company name", "ticker")),
                frozenset({"query"}),
                _arguments(query="Apple", limit=5),
                ("nasdaq", "cboe", "tmx", "sec"),
            ),
        ),
    ),
    Capability(
        "symbol_lookup",
        "Look up one listed symbol from a name or ticker query.",
        (
            Facet(
                "lookup",
                (("lookup", "search"), ("stock symbol", "company name", "ticker")),
                frozenset({"query"}),
                _arguments(query="AAPL", limit=5),
                ("nasdaq", "cboe", "tmx", "sec"),
            ),
        ),
    ),
    Capability(
        "quote",
        "Retrieve a current equity quote.",
        (
            Facet(
                "quote",
                (("quote",), ("stock", "equity")),
                frozenset({"symbol"}),
                _arguments(symbol="AAPL"),
                ("yfinance", "cboe", "tmx"),
            ),
        ),
    ),
    Capability(
        "profile",
        "Retrieve a company profile.",
        (
            Facet(
                "profile",
                (("company",), ("profile", "general information")),
                frozenset({"symbol"}),
                _arguments(symbol="AAPL"),
                ("yfinance", "finviz", "tmx"),
            ),
        ),
    ),
    Capability(
        "history",
        "Retrieve bounded equity OHLCV history.",
        (
            Facet(
                "history",
                (("historical price",), ("stock", "equity")),
                frozenset({"symbol", "start_date", "end_date"}),
                _history_arguments,
                ("yfinance", "cboe", "tmx"),
            ),
        ),
    ),
    Capability(
        "income_statement",
        "Retrieve company income statements.",
        (
            Facet(
                "income",
                (("income statement",),),
                frozenset({"symbol"}),
                _arguments(symbol="AAPL", limit=2),
                ("yfinance",),
            ),
        ),
    ),
    Capability(
        "balance_sheet",
        "Retrieve company balance sheets.",
        (
            Facet(
                "balance",
                (("balance sheet",),),
                frozenset({"symbol"}),
                _arguments(symbol="AAPL", limit=2),
                ("yfinance",),
            ),
        ),
    ),
    Capability(
        "cash_flow_statement",
        "Retrieve company cash-flow statements.",
        (
            Facet(
                "cash_flow",
                (("cash flow statement",),),
                frozenset({"symbol"}),
                _arguments(symbol="AAPL", limit=2),
                ("yfinance",),
            ),
        ),
    ),
    Capability(
        "holders",
        "Retrieve holder ownership and share statistics.",
        (
            Facet(
                "holders",
                (("holder", "ownership", "share float"),),
                frozenset({"symbol"}),
                _arguments(symbol="AAPL"),
                ("yfinance",),
            ),
        ),
    ),
    Capability(
        "analyst_data",
        "Retrieve analyst targets, estimates, or recommendations.",
        (
            Facet(
                "analyst",
                (
                    ("analyst", "consensus"),
                    ("estimate", "price target", "recommendation"),
                ),
                frozenset({"symbol"}),
                _arguments(symbol="AAPL"),
                ("yfinance",),
            ),
        ),
    ),
    Capability(
        "company_calendar",
        "Retrieve company earnings or event calendar data.",
        (
            Facet(
                "calendar",
                (("calendar", "event", "release"), ("earnings", "company event")),
                frozenset({"start_date", "end_date"}),
                _calendar_arguments,
                ("nasdaq", "seeking_alpha", "tmx"),
            ),
        ),
    ),
    Capability(
        "corporate_actions",
        "Retrieve corporate-action fields with historical prices.",
        (
            Facet(
                "actions",
                (("historical price",), ("dividends", "stock splits")),
                frozenset({"symbol", "start_date", "end_date", "include_actions"}),
                _actions_arguments,
                ("yfinance",),
            ),
        ),
    ),
    Capability(
        "option_expirations",
        "List available option expiration dates.",
        (
            Facet(
                "expirations",
                (("options chain", "option chain"),),
                frozenset({"symbol"}),
                _arguments(symbol="AAPL"),
                ("yfinance", "cboe", "tmx"),
            ),
        ),
    ),
    Capability(
        "option_chain",
        "Retrieve an option chain.",
        (
            Facet(
                "chain",
                (("options chain", "option chain"),),
                frozenset({"symbol"}),
                _arguments(symbol="AAPL"),
                ("yfinance", "cboe", "tmx"),
            ),
        ),
    ),
    Capability(
        "etf_fund_data",
        "Retrieve ETF or fund overview data.",
        (
            Facet(
                "fund",
                (("etf information", "fund information", "fund overview"),),
                frozenset({"symbol"}),
                _arguments(symbol="SPY"),
                ("yfinance", "tmx"),
            ),
        ),
    ),
    Capability(
        "sustainability_esg",
        "Retrieve available ESG metrics without overstating provider coverage.",
        (
            Facet(
                "governance_risk",
                (("fundamental metrics",),),
                frozenset({"symbol"}),
                _arguments(symbol="AAPL"),
                ("yfinance",),
                frozenset(
                    {
                        "overall_risk",
                        "audit_risk",
                        "board_risk",
                        "compensation_risk",
                        "shareholder_rights_risk",
                    }
                ),
            ),
            Facet(
                "environmental_social_scores",
                (("environmental score",), ("social score",)),
                frozenset({"symbol"}),
                _arguments(symbol="AAPL"),
                (),
                frozenset(
                    {
                        "esg_score",
                        "environmental_score",
                        "social_score",
                        "governance_score",
                        "total_esg",
                    }
                ),
            ),
        ),
    ),
    Capability(
        "company_news",
        "Retrieve recent company news.",
        (
            Facet(
                "news",
                (("company news",),),
                frozenset({"symbol"}),
                _arguments(symbol="AAPL", limit=3),
                ("yfinance", "tmx"),
            ),
        ),
    ),
    Capability(
        "screener",
        "Screen companies using a keyless OpenBB provider.",
        (
            Facet(
                "screen",
                (("screen for companies", "company screener"),),
                frozenset({"limit"}),
                _arguments(limit=5),
                ("yfinance", "finviz", "nasdaq"),
            ),
        ),
    ),
    Capability(
        "sector_leaders",
        "Return leading companies within a sector.",
        (
            Facet(
                "sector",
                (("screen", "screener"),),
                frozenset({"sector", "limit"}),
                _arguments(sector="technology", limit=5),
                ("yfinance", "finviz", "nasdaq"),
            ),
        ),
    ),
    Capability(
        "peer_comparison",
        "Compare quotes for a caller-supplied peer set.",
        (
            Facet(
                "comparison",
                (("quote",), ("stock", "equity")),
                frozenset({"symbol"}),
                _arguments(symbol="AAPL,MSFT"),
                ("yfinance", "cboe", "tmx"),
            ),
        ),
    ),
)


def provider_ids(tool: dict[str, Any]) -> set[str]:
    """Return declared provider choices from a Tool's top-level input schema."""

    provider = tool.get("inputSchema", {}).get("properties", {}).get("provider", {})
    discovered: set[str] = set()

    def visit(value: object) -> None:
        if isinstance(value, dict):
            const = value.get("const")
            if isinstance(const, str):
                discovered.add(const)
            default = value.get("default")
            if isinstance(default, str):
                discovered.add(default)
            enum = value.get("enum")
            if isinstance(enum, list):
                discovered.update(item for item in enum if isinstance(item, str))
            for nested in value.values():
                visit(nested)
        elif isinstance(value, list):
            for nested in value:
                visit(nested)

    visit(provider)
    return discovered


def has_provider_argument(tool: dict[str, Any]) -> bool:
    """Return whether the Tool schema declares a top-level provider input."""

    properties = tool.get("inputSchema", {}).get("properties", {})
    return isinstance(properties, dict) and "provider" in properties


def _search_text(tool: dict[str, Any]) -> str:
    name = str(tool.get("name", "")).replace("_", " ")
    description = str(tool.get("description", ""))
    schema = json.dumps(
        {
            "input": tool.get("inputSchema", {}),
            "output": tool.get("outputSchema", {}),
        },
        ensure_ascii=False,
    )
    return f"{name} {description} {schema}".lower().replace("_", " ")


def semantic_candidates(
    tools: list[dict[str, Any]], facet: Facet
) -> list[dict[str, Any]]:
    """Rank actual runtime tools by semantics, never by a fixed Tool name."""

    candidates: list[tuple[int, dict[str, Any]]] = []
    for tool in tools:
        text = _search_text(tool)
        if not all(any(term in text for term in group) for group in facet.term_groups):
            continue
        name_text = str(tool.get("name", "")).replace("_", " ").lower()
        score = sum(
            4 if term in name_text else 1
            for group in facet.term_groups
            for term in group
            if term in text
        )
        candidates.append((score, tool))
    ranked = sorted(candidates, key=lambda item: (-item[0], item[1]["name"]))
    return [tool for _, tool in ranked]


def _candidate_record(
    tool: dict[str, Any], facet: Facet | None = None
) -> dict[str, Any]:
    record: dict[str, Any] = {
        "tool": tool["name"],
        "providers": sorted(provider_ids(tool)),
    }
    if facet is not None:
        properties = set(tool.get("inputSchema", {}).get("properties", {}))
        missing = sorted(facet.schema_fields - properties)
        if missing:
            record["missing_schema_fields"] = missing
    return record


def _tool_error(result: dict[str, Any]) -> str:
    content = result.get("content")
    if isinstance(content, list):
        for item in content:
            if isinstance(item, dict) and isinstance(item.get("text"), str):
                return item["text"][:1000]
    return "OpenBB Tool returned isError=true"


def validate_data_result(
    result: dict[str, Any],
    expected_provider: str,
    result_fields_any: frozenset[str] = frozenset(),
) -> tuple[bool, str | None, object]:
    """Validate only the canonical OpenBB OBBject provenance location."""

    if result.get("isError") is True:
        return False, _tool_error(result), None
    structured = result.get("structuredContent")
    if not isinstance(structured, dict) or not structured:
        return False, "Tool returned no structured content", None
    actual_provider = structured.get("provider")
    if actual_provider != expected_provider:
        return (
            False,
            "Tool provenance provider mismatch: "
            f"expected {expected_provider}, got {actual_provider!r}",
            actual_provider,
        )
    available_fields = structured_field_names(structured)
    if result_fields_any and not available_fields & result_fields_any:
        return (
            False,
            "Tool result omitted the required data fields; expected any of "
            f"{sorted(result_fields_any)}",
            None,
        )
    warnings = structured.get("warnings")
    return True, None, warnings


def structured_field_names(value: object) -> set[str]:
    """Collect normalized JSON object keys from structured Tool content."""

    discovered: set[str] = set()
    pending = [value]
    while pending:
        current = pending.pop()
        if isinstance(current, dict):
            discovered.update(str(key).lower() for key in current)
            pending.extend(current.values())
        elif isinstance(current, list):
            pending.extend(current)
    return discovered


def audit_providerless_surface(
    tools: list[dict[str, Any]], manifest: dict[str, Any]
) -> dict[str, Any]:
    """Confirm the scoped runtime exposes no unclassified providerless Tool."""

    providerless = {
        tool["name"]
        for tool in tools
        if tool["name"] not in _ADMIN_TOOLS and not has_provider_argument(tool)
    }
    policy = manifest["providerless_tool_policy"]
    local = set(policy["local_tools"])
    implicit = set(policy["implicit_provider"])
    return {
        "exposed": sorted(providerless),
        "local": sorted(providerless & local),
        "implicit": sorted(providerless & implicit),
        "unknown": sorted(providerless - local - implicit),
    }


def _run_facet(
    client: RuntimeClient,
    tools: list[dict[str, Any]],
    facet: Facet,
    keyless_provider_ids: set[str],
) -> dict[str, Any]:
    candidates = semantic_candidates(tools, facet)
    compatible = [
        tool
        for tool in candidates
        if facet.schema_fields <= set(tool.get("inputSchema", {}).get("properties", {}))
    ]
    eligible = [
        tool for tool in compatible if provider_ids(tool) & keyless_provider_ids
    ]
    record: dict[str, Any] = {
        "facet": facet.id,
        "candidates": [_candidate_record(tool, facet) for tool in candidates[:5]],
    }
    if not eligible:
        reason = (
            "No semantically matching runtime Tool has the required input fields."
            if candidates and not compatible
            else "No semantically matching runtime Tool declares an enabled "
            "keyless provider."
        )
        record.update(
            {
                "status": "not_discovered",
                "reason": reason,
            }
        )
        return record

    attempts: list[dict[str, Any]] = []
    for tool in eligible[:5]:
        schema = tool.get("inputSchema", {})
        declared = provider_ids(tool) & keyless_provider_ids
        preferred = [
            provider for provider in facet.provider_preferences if provider in declared
        ]
        providers = preferred or (
            sorted(declared) if not facet.provider_preferences else []
        )
        for provider in providers:
            arguments = facet.arguments(schema, provider)
            required = set(schema.get("required", []))
            missing = sorted(required - set(arguments))
            attempt: dict[str, Any] = {
                "tool": tool["name"],
                "provider": provider,
                "arguments": arguments,
            }
            if missing:
                attempt.update(
                    {
                        "status": "invalid_arguments",
                        "reason": f"Live probe lacks required arguments: {missing}",
                    }
                )
                attempts.append(attempt)
                continue

            started = time.monotonic()
            try:
                result = client.call_tool(tool["name"], arguments)
            except Exception as error:  # report every capability in one run
                attempt.update(
                    {
                        "status": "call_failed",
                        "reason": str(error)[:1000],
                        "elapsed_ms": round((time.monotonic() - started) * 1000),
                    }
                )
                attempts.append(attempt)
                continue
            valid, reason, warnings = validate_data_result(
                result,
                provider,
                facet.result_fields_any,
            )
            attempt.update(
                {
                    "status": "passed" if valid else "call_failed",
                    "elapsed_ms": round((time.monotonic() - started) * 1000),
                }
            )
            if reason:
                attempt["reason"] = reason
            if warnings:
                attempt["warnings"] = str(warnings)[:1000]
            if facet.result_fields_any:
                structured = result.get("structuredContent", {})
                attempt["matched_result_fields"] = sorted(
                    structured_field_names(structured) & facet.result_fields_any
                )
            attempts.append(attempt)
            if valid:
                record.update(attempt)
                if len(attempts) > 1:
                    record["attempts"] = attempts
                return record

    record["attempts"] = attempts
    record["status"] = "call_failed"
    record["reason"] = (
        attempts[-1].get("reason", "All keyless provider calls failed")
        if attempts
        else "No eligible Tool/provider pair was callable"
    )
    return record


def run_live_parity(bundle: Path, *, timeout: float = 45.0) -> dict[str, Any]:
    """Discover the staged inventory, run keyless calls, and return a report."""

    executable, _manifest_path, manifest = resolve_bundle(bundle)
    started_at = datetime.now(timezone.utc)
    keyless_provider_ids = {
        provider["id"]
        for provider in manifest["providers"]
        if provider.get("keyless") is True
    }
    with RuntimeClient(
        executable,
        manifest,
        enabled_provider_ids=sorted(keyless_provider_ids),
        timeout=timeout,
    ) as client:
        categories, discovered = client.discover_all_tools()
        tools = [tool for tool in discovered if tool.get("name") not in _ADMIN_TOOLS]
        providerless = audit_providerless_surface(discovered, manifest)
        if providerless["unknown"]:
            raise RuntimeError(
                f"runtime exposed unknown providerless tools: {providerless['unknown']}"
            )
        capabilities = []
        for capability in _CAPABILITIES:
            facets = [
                _run_facet(
                    client,
                    tools,
                    facet,
                    keyless_provider_ids,
                )
                for facet in capability.facets
            ]
            passed = sum(facet["status"] == "passed" for facet in facets)
            if passed == len(facets):
                status = "passed"
            elif passed:
                status = "partial"
            else:
                status = "failed"
            capabilities.append(
                {
                    "capability": capability.id,
                    "description": capability.description,
                    "status": status,
                    "facets": facets,
                }
            )

    counts = {
        status: sum(item["status"] == status for item in capabilities)
        for status in ("passed", "partial", "failed")
    }
    keyless_tools = sorted(
        (
            _candidate_record(tool)
            for tool in tools
            if provider_ids(tool) & keyless_provider_ids
        ),
        key=lambda item: item["tool"],
    )
    return {
        "schema_version": "guruterminal-openbb-live-parity/1",
        "started_at": started_at.isoformat(),
        "completed_at": datetime.now(timezone.utc).isoformat(),
        "runtime": {
            "runtime_id": manifest["runtime_id"],
            "packages": manifest["packages"],
            "uv_lock_sha256": manifest["uv_lock_sha256"],
            "executable": executable.name,
        },
        "inventory": {
            "categories": categories,
            "tool_count": len(tools),
            "keyless_provider_ids": sorted(keyless_provider_ids),
            "keyless_tools": keyless_tools,
            "providerless": providerless,
        },
        "summary": counts,
        "capabilities": capabilities,
    }


def deterministic_summary(report: dict[str, Any]) -> dict[str, Any]:
    """Strip volatile live values while preserving the parity decision record."""

    capabilities = []
    for capability in report["capabilities"]:
        facets = []
        for facet in capability["facets"]:
            item = {
                key: facet[key]
                for key in (
                    "facet",
                    "status",
                    "tool",
                    "provider",
                    "matched_result_fields",
                    "reason",
                )
                if key in facet
            }
            if facet["status"] != "passed":
                item["candidates"] = [
                    {
                        key: candidate[key]
                        for key in (
                            "tool",
                            "providers",
                            "missing_schema_fields",
                        )
                        if key in candidate
                    }
                    for candidate in facet.get("candidates", [])
                ]
            facets.append(item)
        capabilities.append(
            {
                "capability": capability["capability"],
                "status": capability["status"],
                "facets": facets,
            }
        )
    runtime = report["runtime"]
    providerless = report["inventory"]["providerless"]
    return {
        "schema_version": "guruterminal-openbb-parity-summary/1",
        "runtime": {
            "runtime_id": runtime["runtime_id"],
            "packages": runtime["packages"],
            "uv_lock_sha256": runtime["uv_lock_sha256"],
        },
        "summary": report["summary"],
        "providerless_unknown": providerless["unknown"],
        "capabilities": capabilities,
    }


def _default_bundle() -> Path:
    project = Path(__file__).resolve().parents[2]
    return project.parent / "src-tauri" / "resources" / "pi-runtime" / "openbb"


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run opt-in keyless OpenBB parity against a staged bundle."
    )
    parser.add_argument("--bundle", type=Path, default=_default_bundle())
    parser.add_argument("--report", type=Path)
    parser.add_argument("--summary-report", type=Path)
    parser.add_argument("--timeout", type=float, default=45.0)
    arguments = parser.parse_args()

    report = run_live_parity(arguments.bundle, timeout=arguments.timeout)
    rendered = json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True)
    if arguments.report:
        arguments.report.parent.mkdir(parents=True, exist_ok=True)
        arguments.report.write_text(rendered + "\n", encoding="utf-8")
    if arguments.summary_report:
        summary = json.dumps(
            deterministic_summary(report),
            ensure_ascii=False,
            indent=2,
            sort_keys=True,
        )
        arguments.summary_report.parent.mkdir(parents=True, exist_ok=True)
        arguments.summary_report.write_text(summary + "\n", encoding="utf-8")
    print(rendered)
    summary = report["summary"]
    return 0 if summary["partial"] == summary["failed"] == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
