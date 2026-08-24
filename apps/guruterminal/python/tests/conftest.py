from __future__ import annotations

from typing import Any


def source(
    source_id: str = "source-1",
    *,
    available_at: str = "2024-11-01T00:00:00Z",
    retrieved_at: str = "2025-01-02T00:00:00Z",
) -> dict[str, Any]:
    return {
        "source_id": source_id,
        "provider": "fixture",
        "as_of": "2024-09-30T00:00:00Z",
        "available_at": available_at,
        "retrieved_at": retrieved_at,
    }


def context(*sources: dict[str, Any]) -> dict[str, Any]:
    return {
        "data_cutoff": "2025-01-01T00:00:00Z",
        "timeout_ms": 30_000,
        "sources": list(sources) if sources else [source()],
    }
