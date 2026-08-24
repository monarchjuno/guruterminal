from __future__ import annotations

import asyncio
import subprocess
import sys
from pathlib import Path

from fastapi import FastAPI
from fastapi.routing import APIRoute
from fastapi.testclient import TestClient

from fastmcp import Client
from fastmcp.exceptions import ToolError
from fastmcp.tools.tool import ToolResult
from openbb_mcp_server.app.app import create_mcp_server
from openbb_mcp_server.models.settings import MCPSettings

from guruterminal_openbb.manifest import (
    load_runtime_manifest,
    resolve_providerless_tool_policy,
)
from guruterminal_openbb.server import (
    _canonicalize_implicit_provider_result,
    mark_registered_tools_read_only,
    retain_read_only_routes,
)


def route_paths(app: FastAPI) -> set[str]:
    return {route.path for route in app.router.routes if isinstance(route, APIRoute)}


def test_route_policy_keeps_queries_and_declared_read_only_analysis() -> None:
    app = FastAPI()

    @app.get("/api/v1/equity/price/quote")
    def quote(provider: str) -> None:
        return None

    @app.post("/api/v1/technical/sma")
    def technical() -> None:
        return None

    @app.post("/api/v1/equity/admin/update")
    def unsafe_post() -> None:
        return None

    @app.get("/api/v1/news/world")
    def disabled_category() -> None:
        return None

    isolated = retain_read_only_routes(
        app,
        {"equity", "technical"},
        "/api/v1",
        ("/api/v1/technical/*",),
        providerless_local_tools={"technical_sma"},
    )

    assert route_paths(isolated) == {
        "/api/v1/equity/price/quote",
        "/api/v1/technical/sma",
    }
    assert route_paths(app) == {
        "/api/v1/equity/price/quote",
        "/api/v1/technical/sma",
        "/api/v1/equity/admin/update",
        "/api/v1/news/world",
    }


def test_providerless_routes_are_manifest_authorized_and_fail_closed() -> None:
    app = FastAPI()

    @app.get("/api/v1/imf_utils/list_dataflows")
    def implicit_imf() -> None:
        return None

    @app.get("/api/v1/coverage/providers")
    def local_metadata() -> None:
        return None

    @app.get("/api/v1/vendor/direct_data")
    def unknown_external() -> None:
        return None

    policy = {
        "providerless_local_tools": {"coverage_providers"},
        "providerless_implicit_provider": {"imf_utils_list_dataflows": "imf"},
    }
    disabled = retain_read_only_routes(
        app,
        {"imf_utils", "coverage", "vendor"},
        "/api/v1",
        enabled_provider_ids=set(),
        **policy,
    )
    assert route_paths(disabled) == {"/api/v1/coverage/providers"}

    enabled = retain_read_only_routes(
        app,
        {"imf_utils", "coverage", "vendor"},
        "/api/v1",
        enabled_provider_ids={"imf"},
        **policy,
    )
    assert route_paths(enabled) == {
        "/api/v1/coverage/providers",
        "/api/v1/imf_utils/list_dataflows",
    }


def test_isolated_router_dispatches_routes_added_after_filtering() -> None:
    app = FastAPI()

    @app.get("/api/v1/equity/quote")
    def quote(provider: str) -> dict[str, str]:
        return {"provider": provider}

    isolated = retain_read_only_routes(app, {"equity"}, "/api/v1")

    @isolated.get("/api/v1/equity/late_route")
    def late_route() -> dict[str, bool]:
        return {"ok": True}

    response = TestClient(isolated).get("/api/v1/equity/late_route")
    assert response.status_code == 200
    assert response.json() == {"ok": True}


def test_implicit_provider_receipt_is_canonical_and_contradictions_fail() -> None:
    result = ToolResult(
        structured_content={"provider": None, "results": [{"id": "flow"}]}
    )
    canonical = _canonicalize_implicit_provider_result(result, "imf")
    assert canonical.structured_content == {
        "provider": "imf",
        "results": [{"id": "flow"}],
    }

    contradictory = ToolResult(structured_content={"provider": "other", "results": []})
    try:
        _canonicalize_implicit_provider_result(contradictory, "imf")
    except RuntimeError as error:
        assert "contradicted" in str(error)
    else:
        raise AssertionError("contradictory implicit provider was accepted")


def test_dynamic_mcp_call_enforces_manifest_implicit_provider_contract() -> None:
    manifest = load_runtime_manifest()
    _, implicit_provider = resolve_providerless_tool_policy(manifest)
    tool_name = "imf_utils_list_dataflows"
    manifest_provider = implicit_provider[tool_name]

    app = FastAPI()

    @app.get("/api/v1/imf_utils/list_dataflows")
    def list_dataflows(contradict: bool = False) -> dict[str, object]:
        return {
            "provider": "unexpected" if contradict else None,
            "results": [{"id": "flow"}],
        }

    settings = MCPSettings(
        name="Guru implicit-provider contract test",
        description="Deterministic in-process OpenAPI Tool contract.",
        default_tool_categories=[],
        allowed_tool_categories=["imf_utils"],
        enable_tool_discovery=True,
        describe_responses=False,
        instructions=None,
        system_prompt_file=None,
        server_prompts_file=None,
        default_skills_dir=None,
        skills_providers=None,
        dependencies=None,
        mask_error_details=True,
        list_page_size=100,
        module_exclusion_map={"__guruterminal_none__": "__guruterminal_none__"},
    )
    server = create_mcp_server(settings, app)
    mark_registered_tools_read_only(server, {tool_name: manifest_provider})

    async def call_through_mcp_session() -> None:
        async with Client(server) as client:
            assert tool_name not in {tool.name for tool in await client.list_tools()}

            activation = await client.call_tool(
                "activate_tools", {"tool_names": [tool_name]}
            )
            assert activation.is_error is False
            assert tool_name in {tool.name for tool in await client.list_tools()}

            result = await client.call_tool(tool_name, {})
            assert result.is_error is False
            assert result.structured_content == {
                "provider": manifest_provider,
                "results": [{"id": "flow"}],
            }

            try:
                await client.call_tool(tool_name, {"contradict": True})
            except ToolError:
                pass
            else:
                raise AssertionError(
                    "MCP accepted provider metadata that contradicted the manifest"
                )

    asyncio.run(call_through_mcp_session())


def test_official_server_starts_admin_only_without_prompts_or_resources(
    tmp_path: Path,
) -> None:
    tmp_path.chmod(0o700)
    script = """
import asyncio
import sys
from pathlib import Path
from guruterminal_openbb.bootstrap import (
    Bootstrap,
    BootstrapSettings,
    configure_scratch_environment,
)
from guruterminal_openbb.manifest import (
    load_runtime_manifest,
    resolve_network_hosts,
    resolve_providerless_tool_policy,
)

scratch = Path(sys.argv[1])
configure_scratch_environment(scratch)
from guruterminal_openbb.server import create_server

manifest = load_runtime_manifest()
categories = tuple(manifest["allowed_categories"])
providers = tuple(provider["id"] for provider in manifest["providers"])
network_hosts = tuple(sorted(resolve_network_hosts(set(providers), manifest)))
server = create_server(
    Bootstrap(
        run_id="admin-surface-test",
        scratch_dir=scratch,
        credentials={},
        settings=BootstrapSettings(
            allowed_categories=categories,
            enabled_provider_ids=providers,
            allowed_network_hosts=network_hosts,
            provider_config={},
        ),
    )
)

async def check_surface():
    tools = await server.list_tools()
    assert {tool.name for tool in tools} == {
        "available_categories",
        "available_tools",
        "activate_tools",
        "deactivate_tools",
        "activate_category",
    }
    assert await server.list_prompts() == []
    assert await server.list_resources() == []
    assert await server.list_resource_templates() == []

    category_result = await server.call_tool("available_categories", {})
    discovered_categories = {
        item["name"] for item in category_result.structured_content["result"]
    }
    assert discovered_categories == set(categories)

    discovered_tools = set()
    for category in categories:
        result = await server.call_tool("available_tools", {"category": category})
        discovered_tools.update(
            item["name"] for item in result.structured_content["result"]
        )
    probes = [
        provider["verification_probe"]
        for provider in manifest["providers"]
        if provider.get("verification_probe")
    ]
    assert all(probe["tool"] in discovered_tools for probe in probes)

    registered = [
        tool
        for provider in server.providers
        for tool in getattr(provider, "_tools", {}).values()
    ]
    assert registered
    assert all(tool.annotations.readOnlyHint is True for tool in registered)
    assert all(tool.annotations.destructiveHint is False for tool in registered)
    _, implicit_provider = resolve_providerless_tool_policy(manifest)
    registered_by_name = {tool.name: tool for tool in registered}
    assert set(implicit_provider) <= set(registered_by_name)
    for tool_name, provider_id in implicit_provider.items():
        assert getattr(
            registered_by_name[tool_name].run,
            "__guruterminal_implicit_provider__",
        ) == provider_id

    from openbb_core.provider.utils import helpers as provider_helpers
    try:
        await provider_helpers.amake_request("https://www.sec.gov/data.json")
    except PermissionError as error:
        assert "contact email" in str(error)
    else:
        raise AssertionError("unconfigured SEC request was not blocked")

asyncio.run(check_surface())
"""
    completed = subprocess.run(
        [sys.executable, "-c", script, str(tmp_path)],
        check=False,
        capture_output=True,
        text=True,
        timeout=30,
    )
    assert completed.returncode == 0, completed.stderr
