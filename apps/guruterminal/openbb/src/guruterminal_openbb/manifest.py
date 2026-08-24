"""Runtime manifest loading and bootstrap authorization."""

from __future__ import annotations

import json
from importlib.resources import files
from pathlib import Path
from typing import Any

from guruterminal_openbb.bootstrap import Bootstrap, BootstrapError


def manifest_path() -> Path:
    """Return the packaged manifest path in source and frozen builds."""

    packaged = files("guruterminal_openbb").joinpath("runtime-manifest.json")
    if packaged.is_file():
        return Path(str(packaged))
    return Path(__file__).resolve().parents[2] / "runtime-manifest.json"


def load_runtime_manifest(path: Path | None = None) -> dict[str, Any]:
    """Read the non-secret, build-pinned runtime manifest."""

    target = path or manifest_path()
    with target.open(encoding="utf-8") as stream:
        value = json.load(stream)
    if not isinstance(value, dict):
        raise ValueError("OpenBB runtime manifest must be a JSON object")
    return value


def authorize_bootstrap(bootstrap: Bootstrap, manifest: dict[str, Any]) -> Bootstrap:
    """Reject categories, providers, or credential keys outside the manifest."""

    manifest_categories = set(manifest.get("allowed_categories", []))
    requested_categories = set(bootstrap.settings.allowed_categories)
    if not requested_categories:
        raise BootstrapError("bootstrap must allow at least one OpenBB category")
    if not requested_categories <= manifest_categories:
        raise BootstrapError("bootstrap requests an unknown OpenBB category")

    providers = manifest.get("providers")
    if not isinstance(providers, list):
        raise ValueError("OpenBB runtime manifest providers must be an array")
    provider_map = {
        provider["id"]: provider
        for provider in providers
        if isinstance(provider, dict) and isinstance(provider.get("id"), str)
    }
    requested_providers = set(bootstrap.settings.enabled_provider_ids)
    if not requested_providers <= set(provider_map):
        raise BootstrapError("bootstrap enables an unknown OpenBB provider")

    expected_hosts = resolve_network_hosts(requested_providers, manifest)
    if set(bootstrap.settings.allowed_network_hosts) != expected_hosts:
        raise BootstrapError(
            "bootstrap allowed_network_hosts must exactly match the enabled provider host union"
        )

    configured_providers = set(bootstrap.settings.provider_config)
    if not configured_providers <= requested_providers:
        raise BootstrapError("bootstrap configures a disabled OpenBB provider")
    for provider_id, values in bootstrap.settings.provider_config.items():
        mapping = provider_map[provider_id].get("config_mapping", {})
        if not isinstance(mapping, dict):
            raise ValueError("provider config_mapping must be an object")
        if not set(values) <= set(mapping):
            raise BootstrapError("bootstrap contains an unknown provider config field")

    allowed_credentials: set[str] = set()
    for provider_id in requested_providers:
        mapping = provider_map[provider_id].get("credential_mapping", {})
        if not isinstance(mapping, dict):
            raise ValueError("provider credential_mapping must be an object")
        allowed_credentials.update(
            value for value in mapping.values() if isinstance(value, str)
        )
    if not set(bootstrap.credentials) <= allowed_credentials:
        raise BootstrapError("bootstrap contains a credential for a disabled provider")
    return bootstrap


def resolve_network_hosts(provider_ids: set[str], manifest: dict[str, Any]) -> set[str]:
    """Resolve the exact HTTPS host union declared for provider grants."""

    providers = manifest.get("providers")
    if not isinstance(providers, list):
        raise ValueError("OpenBB runtime manifest providers must be an array")
    provider_map = {
        provider["id"]: provider
        for provider in providers
        if isinstance(provider, dict) and isinstance(provider.get("id"), str)
    }
    if not provider_ids <= set(provider_map):
        raise ValueError("network host resolution references an unknown provider")

    result: set[str] = set()
    for provider_id in provider_ids:
        hosts = provider_map[provider_id].get("network_hosts")
        if (
            not isinstance(hosts, list)
            or not hosts
            or not all(isinstance(host, str) and host for host in hosts)
        ):
            raise ValueError(
                f"provider {provider_id} must declare non-empty network_hosts"
            )
        if len(hosts) != len(set(hosts)):
            raise ValueError(f"provider {provider_id} has duplicate network_hosts")
        result.update(hosts)
    return result


def resolve_provider_config(
    bootstrap: Bootstrap, manifest: dict[str, Any]
) -> dict[str, str]:
    """Map Marketplace-facing config names to OpenBB runtime field names."""

    providers = {provider["id"]: provider for provider in manifest["providers"]}
    resolved: dict[str, str] = {}
    for provider_id, values in bootstrap.settings.provider_config.items():
        mapping = providers[provider_id].get("config_mapping", {})
        for source, value in values.items():
            target = mapping[source]
            if target in resolved:
                raise ValueError(f"duplicate OpenBB provider config target: {target}")
            resolved[target] = value
    return resolved


def resolve_providerless_tool_policy(
    manifest: dict[str, Any],
) -> tuple[set[str], dict[str, str]]:
    """Return the audited authority policy for tools without ``provider``.

    OpenBB normally exposes provider selection as a top-level Tool argument.
    A small number of local analytics omit it legitimately, while a few direct
    extension routers have one implicit provider.  Any unclassified route must
    remain hidden so a future extension cannot bypass Guru's provider grants.
    """

    policy = manifest.get("providerless_tool_policy")
    if not isinstance(policy, dict) or set(policy) != {
        "local_tools",
        "implicit_provider",
    }:
        raise ValueError(
            "OpenBB manifest providerless_tool_policy must contain only "
            "local_tools and implicit_provider"
        )
    local_value = policy["local_tools"]
    implicit_value = policy["implicit_provider"]
    if not isinstance(local_value, list) or not all(
        isinstance(item, str) and item for item in local_value
    ):
        raise ValueError("providerless local_tools must be an array of names")
    if len(local_value) != len(set(local_value)):
        raise ValueError("providerless local_tools contains duplicates")
    if not isinstance(implicit_value, dict) or not all(
        isinstance(tool, str) and tool and isinstance(provider, str) and provider
        for tool, provider in implicit_value.items()
    ):
        raise ValueError("providerless implicit_provider must map tools to providers")

    local_tools = set(local_value)
    implicit_provider = dict(implicit_value)
    if local_tools & set(implicit_provider):
        raise ValueError("providerless tools cannot be both local and provider-backed")
    declared_providers = {
        provider.get("id")
        for provider in manifest.get("providers", [])
        if isinstance(provider, dict)
    }
    if not set(implicit_provider.values()) <= declared_providers:
        raise ValueError("providerless policy references an undeclared provider")
    return local_tools, implicit_provider
