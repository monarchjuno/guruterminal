from __future__ import annotations

import hashlib
import json
import re
from importlib.metadata import entry_points, version
from pathlib import Path

from guruterminal_openbb.manifest import (
    load_runtime_manifest,
    resolve_network_hosts,
    resolve_providerless_tool_policy,
)


def _normalized_provider_id(entry_point_name: str) -> str:
    return {
        "famafranch": "famafrench",
        "us_eia": "eia",
    }.get(entry_point_name, entry_point_name)


def test_manifest_matches_pinned_runtime_and_provider_entry_points() -> None:
    manifest = load_runtime_manifest()
    assert manifest["schema_version"] == "guruterminal-mcp-runtime/1"
    assert manifest["runtime_id"] == "openbb"
    assert manifest["executable"] == "guruterminal-openbb"
    assert manifest["protocol"]["initial_tools"] == "admin_only"
    assert set(manifest["protocol"]["control_tool_names"]) == {
        "activate_category",
        "activate_tools",
        "available_categories",
        "available_tools",
        "deactivate_tools",
    }
    assert (
        manifest["protocol"]["provider_receipt_pointer"]
        == "/structuredContent/provider"
    )
    assert manifest["protocol"]["tool_activation"] == {
        "tool_name": "activate_tools",
        "argument_name": "tool_names",
    }
    assert manifest["security"]["read_only"] is True
    lock = Path(__file__).resolve().parents[1] / "uv.lock"
    assert manifest["uv_lock_sha256"] == hashlib.sha256(lock.read_bytes()).hexdigest()

    for package, expected in manifest["packages"].items():
        assert version(package) == expected

    providers = manifest["providers"]
    provider_ids = [provider["id"] for provider in providers]
    assert len(provider_ids) == len(set(provider_ids)) == 32
    discovered = {
        _normalized_provider_id(item.name): (item.dist.name, item.dist.version)
        for item in entry_points(group="openbb_provider_extension")
    }
    declared = {
        provider["id"]: (provider["package"], provider["version"])
        for provider in providers
    }
    assert declared == discovered


def test_manifest_credentials_and_probes_are_provider_explicit() -> None:
    providers = load_runtime_manifest()["providers"]
    credential_keys: set[str] = set()
    for provider in providers:
        mapping = provider["credential_mapping"]
        assert all(value not in credential_keys for value in mapping.values())
        credential_keys.update(mapping.values())
        probe = provider.get("verification_probe")
        if probe:
            assert probe["arguments"]["provider"] == provider["id"]

    assert credential_keys == {
        "alpha_vantage_api_key",
        "benzinga_api_key",
        "biztoc_api_key",
        "bls_api_key",
        "cftc_app_token",
        "congress_gov_api_key",
        "econdb_api_key",
        "eia_api_key",
        "fmp_api_key",
        "fred_api_key",
        "intrinio_api_key",
        "nasdaq_api_key",
        "tiingo_token",
        "tradier_api_key",
        "tradingeconomics_api_key",
    }
    keyless_upgrades = {
        provider["id"]
        for provider in providers
        if provider["keyless"] and provider["credential_mapping"]
    }
    assert keyless_upgrades == {"bls", "cftc", "nasdaq"}
    config_mappings = {
        provider["id"]: provider.get("config_mapping", {})
        for provider in providers
        if provider.get("config_mapping")
    }
    assert config_mappings == {
        "sec": {"contact_email": "sec_contact_email"},
        "tradier": {"account_type": "tradier_account_type"},
    }


def test_manifest_declares_bounded_exact_network_hosts_per_provider() -> None:
    providers = load_runtime_manifest()["providers"]
    hostname = re.compile(
        r"^(?=.{1,253}\Z)(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+"
        r"[a-z](?:[a-z0-9-]{0,61}[a-z0-9])?$"
    )
    for provider in providers:
        hosts = provider["network_hosts"]
        assert hosts == sorted(set(hosts))
        assert all(hostname.fullmatch(host) for host in hosts)
        assert "*" not in hosts


def test_marketplace_openbb_hosts_exactly_match_provider_unions() -> None:
    manifest = load_runtime_manifest()
    connectors_root = (
        Path(__file__).resolve().parents[2]
        / "marketplace"
        / "plugins"
        / "openbb"
        / "connectors"
    )
    entries = [
        json.loads(path.read_text(encoding="utf-8"))
        for path in sorted(connectors_root.glob("*.json"))
    ]
    declared_provider_ids: set[str] = set()
    for entry in entries:
        runtime = entry["runtime"]
        assert runtime["server_id"] == "openbb"
        provider_ids = set(runtime["provider_ids"])
        declared_provider_ids.update(provider_ids)
        assert set(entry["permissions"]["network_hosts"]) == resolve_network_hosts(
            provider_ids, manifest
        )

    assert declared_provider_ids == {
        provider["id"] for provider in manifest["providers"]
    }


def test_manifest_audits_every_providerless_tool() -> None:
    manifest = load_runtime_manifest()
    local_tools, implicit_provider = resolve_providerless_tool_policy(manifest)

    assert len(local_tools) == 64
    assert len(implicit_provider) == 9
    assert set(implicit_provider.values()) == {"famafrench", "imf"}
    assert implicit_provider["quantitative_capm"] == "famafrench"
    assert local_tools.isdisjoint(implicit_provider)


def test_wrapper_has_no_provider_specific_compatibility_client() -> None:
    source_root = Path(__file__).resolve().parents[1] / "src" / "guruterminal_openbb"
    source = "\n".join(
        path.read_text(encoding="utf-8") for path in sorted(source_root.glob("*.py"))
    )

    assert "openbb_yfinance" not in source
    assert "import yfinance" not in source
    assert "from yfinance" not in source
    assert "query1.finance.yahoo.com" not in source
    assert "query2.finance.yahoo.com" not in source
    assert "supplemental_tools" not in load_runtime_manifest()
