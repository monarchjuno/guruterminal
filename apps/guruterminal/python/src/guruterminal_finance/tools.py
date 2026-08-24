"""Closed finance-tool registry and execution entry point."""

from __future__ import annotations

import threading
import time
from typing import Any

from .calculations import CALCULATION_FUNCTIONS, CALCULATION_SPECS
from .errors import invalid_params
from .schemas import (
    DataContext,
    WORKER_VERSION,
    canonical_sha256,
    format_datetime,
)
from .tool_runtime import ExecutionControl, ProgressCallback

TOOL_SPECS = {spec.name: spec for spec in CALCULATION_SPECS}
_TOOL_FUNCTIONS = CALCULATION_FUNCTIONS


def list_tools() -> list[dict[str, Any]]:
    return [TOOL_SPECS[name].to_json() for name in sorted(TOOL_SPECS)]


def execute_tool(
    name: object,
    arguments: object,
    context_value: object,
    *,
    cancel_event: threading.Event,
    progress_callback: ProgressCallback,
) -> dict[str, Any]:
    if not isinstance(name, str) or name not in TOOL_SPECS:
        raise invalid_params("Unknown or forbidden tool", path="params.name")
    spec = TOOL_SPECS[name]
    context = DataContext.from_json(
        context_value, allow_future_sources=spec.allow_future_sources
    )
    control = ExecutionControl(
        cancel_event=cancel_event,
        deadline=time.monotonic() + context.timeout_ms / 1000,
        progress_callback=progress_callback,
    )
    control.checkpoint()
    computation = _TOOL_FUNCTIONS[name](arguments, context, control)
    used_sources = [
        source.to_json()
        for source in context.sources
        if source.source_id in computation.used_source_ids
    ]
    input_digest = canonical_sha256(
        {
            "tool": name,
            "tool_version": spec.version,
            "arguments": arguments,
            "context": context.to_json(),
        }
    )
    return {
        "tool": name,
        "tool_version": spec.version,
        "data": computation.data,
        "provenance": {
            "data_cutoff": format_datetime(context.data_cutoff),
            "sources": used_sources,
            "input_sha256": input_digest,
            "worker_version": WORKER_VERSION,
        },
        "warnings": list(computation.warnings),
    }
