"""Guru's read-only wrapper around the official OpenBB MCP server."""

from __future__ import annotations

import copy
import re
import sys
from collections.abc import Callable
from fnmatch import fnmatchcase
from functools import wraps
from typing import Any
from urllib.parse import urlsplit

from guruterminal_openbb.bootstrap import (
    Bootstrap,
    BootstrapError,
    configure_scratch_environment,
    read_bootstrap,
)
from guruterminal_openbb.manifest import (
    authorize_bootstrap,
    load_runtime_manifest,
    resolve_provider_config,
    resolve_providerless_tool_policy,
)
from guruterminal_openbb.network import apply_network_policy

_DISABLED_COMPONENTS = {
    "install_skill",
    "list_prompts",
    "get_prompt",
    "list_resources",
    "read_resource",
}


def _category_for_path(path: str, api_prefix: str) -> str | None:
    normalized_prefix = "/" + api_prefix.strip("/")
    normalized_path = "/" + path.strip("/")
    if normalized_prefix != "/" and normalized_path.startswith(normalized_prefix + "/"):
        normalized_path = normalized_path[len(normalized_prefix) :]
    segments = [part for part in normalized_path.split("/") if part]
    return segments[0] if segments else None


def _tool_name_for_path(path: str, api_prefix: str) -> str:
    """Match OpenBB MCP's path-derived Tool names for authority lookup."""

    normalized_prefix = "/" + api_prefix.strip("/")
    normalized_path = "/" + path.strip("/")
    if normalized_prefix != "/" and normalized_path.startswith(normalized_prefix + "/"):
        normalized_path = normalized_path[len(normalized_prefix) :]
    parts = []
    for segment in normalized_path.split("/"):
        normalized = re.sub(r"[^A-Za-z0-9]+", "_", segment).strip("_")
        if normalized:
            parts.append(normalized.lower())
    return "_".join(parts)


def _route_declares_provider(route: Any) -> bool:
    """Inspect FastAPI dependencies for the top-level provider argument."""

    pending = [route.dependant]
    visited: set[int] = set()
    parameter_groups = (
        "path_params",
        "query_params",
        "header_params",
        "cookie_params",
        "body_params",
    )
    while pending:
        dependant = pending.pop()
        identity = id(dependant)
        if identity in visited:
            continue
        visited.add(identity)
        for group in parameter_groups:
            for parameter in getattr(dependant, group, ()):
                if getattr(parameter, "name", None) == "provider":
                    return True
        pending.extend(getattr(dependant, "dependencies", ()))
    return False


def retain_read_only_routes(
    app: Any,
    allowed_categories: set[str],
    api_prefix: str,
    read_only_post_routes: tuple[str, ...] = (),
    *,
    enabled_provider_ids: set[str] | None = None,
    providerless_local_tools: set[str] | None = None,
    providerless_implicit_provider: dict[str, str] | None = None,
) -> Any:
    """Copy an OpenBB app and retain only authorized read-only routes.

    Providerless routes fail closed unless the runtime manifest classifies the
    exact Tool as local or maps it to an enabled implicit provider.
    """

    from fastapi.routing import APIRoute  # imported only after scratch isolation

    isolated = copy.copy(app)
    isolated.router = copy.copy(app.router)
    enabled_providers = enabled_provider_ids or set()
    local_tools = providerless_local_tools or set()
    implicit_providers = providerless_implicit_provider or {}
    retained = []
    for route in app.router.routes:
        if not isinstance(route, APIRoute):
            continue
        methods = {str(method).upper() for method in route.methods or set()}
        effective_methods = methods - {"HEAD", "OPTIONS"}
        category = _category_for_path(route.path or "", api_prefix)
        method_allowed = effective_methods == {"GET"} or (
            effective_methods == {"POST"}
            and any(
                fnmatchcase(route.path or "", pattern)
                for pattern in read_only_post_routes
            )
        )
        tool_name = _tool_name_for_path(route.path or "", api_prefix)
        provider_allowed = _route_declares_provider(route)
        if not provider_allowed and tool_name in local_tools:
            provider_allowed = True
        if not provider_allowed:
            implicit_provider = implicit_providers.get(tool_name)
            provider_allowed = implicit_provider in enabled_providers
        if method_allowed and category in allowed_categories and provider_allowed:
            retained.append(route)
    isolated.router.routes = retained
    # APIRouter stores ``middleware_stack`` as a bound method. A shallow copy
    # otherwise keeps dispatching through the source router even though its
    # public ``routes`` attribute has been replaced.
    isolated.router.middleware_stack = isolated.router.app
    # ``copy.copy`` may retain a middleware stack already bound to the source
    # application's router. Rebuild it lazily before the isolated ASGI app is
    # passed to FastMCP.
    isolated.middleware_stack = None
    return isolated


def apply_credentials(credentials: dict[str, str]) -> None:
    """Install credentials in both OpenBB in-process command contexts.

    The official REST wrapper normally reloads ``user_settings.json`` for every
    request. Guru must never persist secrets there, so the process-local service
    returns a deep copy of the bootstrapped settings instead.
    """

    from openbb import obb
    from openbb_core.app.service.user_service import UserService

    credentials_model = type(obb.user.credentials).model_validate(credentials)
    obb.user.credentials = credentials_model
    ephemeral_settings = obb.user.model_copy(deep=True)

    def read_ephemeral_settings(_cls: type, _path: Any = None) -> Any:
        return ephemeral_settings.model_copy(deep=True)

    UserService.read_from_file = classmethod(read_ephemeral_settings)
    UserService().default_user_settings = ephemeral_settings.model_copy(deep=True)


def apply_sec_contact_email(contact_email: str | None) -> None:
    """Force Guru's contact identity onto every bundled SEC request path.

    OpenBB SEC 1.6.7 has three module-level header dictionaries and one route
    that builds a placeholder header inside its request function.  Updating the
    dictionaries handles the normal paths; wrapping OpenBB's process-local
    request helpers also covers local headers and SEC calls that omit headers.
    Without configured contact information, the same wrappers fail closed
    before making any SEC request. No installed source or settings are changed.
    """

    from openbb_core.provider.utils import helpers as provider_helpers
    from openbb_sec.utils import definitions, form4

    user_agent = (
        f"Guru Terminal {contact_email}"
        if contact_email
        else "Guru Terminal SEC contact required"
    )
    definitions.HEADERS["User-Agent"] = user_agent
    definitions.SEC_HEADERS["User-Agent"] = user_agent
    form4.SEC_HEADERS["User-Agent"] = user_agent

    previous_async = provider_helpers.amake_request
    previous_sync = provider_helpers.make_request
    original_async = getattr(
        previous_async, "__guruterminal_sec_original__", previous_async
    )
    original_sync = getattr(
        previous_sync, "__guruterminal_sec_original__", previous_sync
    )

    @wraps(original_async)
    async def sec_aware_async_request(url: str, *args: Any, **kwargs: Any) -> Any:
        if _is_sec_url(url):
            if contact_email is None:
                raise PermissionError("SEC requests require a configured contact email")
            kwargs["headers"] = _headers_with_identity(
                kwargs.get("headers"), user_agent
            )
        return await original_async(url, *args, **kwargs)

    @wraps(original_sync)
    def sec_aware_sync_request(url: str, *args: Any, **kwargs: Any) -> Any:
        if _is_sec_url(url):
            if contact_email is None:
                raise PermissionError("SEC requests require a configured contact email")
            kwargs["headers"] = _headers_with_identity(
                kwargs.get("headers"), user_agent
            )
        return original_sync(url, *args, **kwargs)

    setattr(sec_aware_async_request, "__guruterminal_sec_original__", original_async)
    setattr(sec_aware_sync_request, "__guruterminal_sec_original__", original_sync)
    provider_helpers.amake_request = sec_aware_async_request
    provider_helpers.make_request = sec_aware_sync_request

    # OpenBB imports some helpers at module import time. Replace only aliases
    # that point at the exact old/original helper; unrelated provider code is
    # intentionally untouched.
    for module_name, module in tuple(sys.modules.items()):
        if not module_name.startswith("openbb_sec.") or module is None:
            continue
        if getattr(module, "amake_request", None) in {previous_async, original_async}:
            setattr(module, "amake_request", sec_aware_async_request)
        if getattr(module, "make_request", None) in {previous_sync, original_sync}:
            setattr(module, "make_request", sec_aware_sync_request)


def _is_sec_url(url: object) -> bool:
    """Return whether a request target belongs to the SEC HTTPS origin."""

    if not isinstance(url, str):
        return False
    parsed = urlsplit(url)
    host = (parsed.hostname or "").lower()
    return parsed.scheme.lower() == "https" and (
        host == "sec.gov" or host.endswith(".sec.gov")
    )


def _headers_with_identity(headers: object, user_agent: str) -> dict[str, Any]:
    """Copy request headers and replace any case variant of User-Agent."""

    try:
        updated = dict(headers or {})  # type: ignore[arg-type]
    except (TypeError, ValueError):
        updated = {}
    for key in tuple(updated):
        if str(key).lower() == "user-agent":
            del updated[key]
    updated["User-Agent"] = user_agent
    return updated


def _canonicalize_implicit_provider_result(result: Any, provider: str) -> Any:
    """Stamp the signed provider identity onto a providerless route result."""

    from fastmcp.tools.tool import ToolResult

    if result.is_error:
        return result
    structured = result.structured_content
    if not isinstance(structured, dict):
        raise RuntimeError("implicit-provider Tool returned no structured content")
    reported = structured.get("provider")
    if reported not in {None, provider}:
        raise RuntimeError("implicit-provider Tool contradicted its runtime manifest")
    canonical = dict(structured)
    canonical["provider"] = provider
    return ToolResult(
        structured_content=canonical,
        meta=result.meta,
        is_error=result.is_error,
    )


def mark_registered_tools_read_only(
    server: Any, implicit_provider: dict[str, str]
) -> int:
    """Attach MCP safety annotations to every retained OpenAPI Tool.

    OpenBB MCP 1.4.1 does not emit Tool annotations.  Guru derives this hint
    only after ``retain_read_only_routes`` has discarded every route outside
    the manifest's GET/read-only-POST policy.
    """

    from fastmcp.server.providers.openapi.components import OpenAPITool
    from mcp.types import ToolAnnotations

    annotations = ToolAnnotations(readOnlyHint=True, destructiveHint=False)
    marked = 0
    stamped: set[str] = set()
    for provider in server.providers:
        tools = getattr(provider, "_tools", None)
        if not isinstance(tools, dict):
            continue
        for tool in tools.values():
            if isinstance(tool, OpenAPITool):
                tool.annotations = annotations.model_copy(deep=True)
                if provider_id := implicit_provider.get(tool.name):
                    original_run = tool.run

                    @wraps(original_run)
                    async def run_with_provider(
                        arguments: dict[str, Any],
                        *,
                        _original_run: Callable[[dict[str, Any]], Any] = original_run,
                        _provider_id: str = provider_id,
                    ) -> Any:
                        result = await _original_run(arguments)
                        return _canonicalize_implicit_provider_result(
                            result, _provider_id
                        )

                    setattr(
                        run_with_provider,
                        "__guruterminal_implicit_provider__",
                        provider_id,
                    )
                    # OpenAPITool is a Pydantic model and rejects ordinary
                    # assignment to methods. This is a process-local wrapper
                    # on a trusted, frozen instance; no installed code changes.
                    object.__setattr__(tool, "run", run_with_provider)
                    stamped.add(tool.name)
                marked += 1
    if marked == 0:
        raise RuntimeError("OpenBB MCP created no read-only OpenAPI Tools")
    if stamped != set(implicit_provider):
        raise RuntimeError("OpenBB MCP implicit-provider inventory is incomplete")
    return marked


def create_server(bootstrap: Bootstrap) -> Any:
    """Create the official OpenBB FastMCP instance under Guru restrictions."""

    manifest = load_runtime_manifest()
    local_tools, implicit_provider = resolve_providerless_tool_policy(manifest)
    provider_config = resolve_provider_config(bootstrap, manifest)
    credentials = bootstrap.credentials.copy()
    if account_type := provider_config.get("tradier_account_type"):
        if account_type not in {"sandbox", "live"}:
            raise BootstrapError("tradier account_type must be sandbox or live")
        credentials["tradier_account_type"] = account_type
    # Install the process-local egress guard before importing the OpenBB app,
    # provider registry, or FastMCP. Provider modules that bind OpenBB request
    # helpers during import therefore receive the guarded functions.
    apply_network_policy(bootstrap.settings.allowed_network_hosts)

    from openbb_core.api.rest_api import app as openbb_app
    from openbb_core.app.service.system_service import SystemService
    from openbb_mcp_server.app.app import create_mcp_server
    from openbb_mcp_server.models.settings import MCPSettings

    apply_credentials(credentials)
    credentials.clear()
    apply_sec_contact_email(provider_config.get("sec_contact_email"))
    security = manifest.get("security", {})
    read_only_post_routes = tuple(security.get("read_only_post_routes", []))
    api_prefix = SystemService().system_settings.api_settings.prefix or ""
    target_app = retain_read_only_routes(
        openbb_app,
        set(bootstrap.settings.allowed_categories),
        api_prefix,
        read_only_post_routes,
        enabled_provider_ids=set(bootstrap.settings.enabled_provider_ids),
        providerless_local_tools=local_tools,
        providerless_implicit_provider=implicit_provider,
    )
    settings = MCPSettings(
        name="Guru Terminal OpenBB",
        description="Read-only OpenBB tools exposed through Guru Terminal.",
        default_tool_categories=[],
        allowed_tool_categories=list(bootstrap.settings.allowed_categories),
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
        # The upstream server excludes its local analytics modules by default.
        # Guru explicitly permits their declared read-only POST routes, so use
        # a non-matching sentinel map instead of the upstream defaults.
        module_exclusion_map={"__guruterminal_none__": "__guruterminal_none__"},
    )
    server = create_mcp_server(settings, target_app)
    enabled_provider_ids = set(bootstrap.settings.enabled_provider_ids)
    mark_registered_tools_read_only(
        server,
        {
            tool_name: provider_id
            for tool_name, provider_id in implicit_provider.items()
            if provider_id in enabled_provider_ids
        },
    )
    server.disable(names=_DISABLED_COMPONENTS)
    return server


def run(
    *,
    stdin: Any | None = None,
    server_factory: Callable[[Bootstrap], Any] = create_server,
) -> int:
    """Bootstrap and run the official MCP stdio loop."""

    source = stdin if stdin is not None else sys.stdin.buffer
    bootstrap = read_bootstrap(source)
    manifest = load_runtime_manifest()
    authorize_bootstrap(bootstrap, manifest)
    configure_scratch_environment(bootstrap.scratch_dir)
    try:
        server = server_factory(bootstrap)
    finally:
        bootstrap.credentials.clear()
    server.run("stdio")
    return 0


def main() -> int:
    """Console entry point; errors never echo bootstrap data or credentials."""

    try:
        return run()
    except BootstrapError as error:
        print(f"OpenBB bootstrap rejected: {error}", file=sys.stderr)
        return 78
    except KeyboardInterrupt:
        return 130
