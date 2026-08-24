"""Validated protocol and provenance schemas.

The worker intentionally has no runtime dependency on a general validation
framework. These small immutable records are the complete accepted surface.
"""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from datetime import datetime, timezone
from importlib.metadata import PackageNotFoundError, version
from typing import Any, Mapping

from .errors import invalid_params, invalid_context

PROTOCOL_VERSION = "1"

try:
    WORKER_VERSION = version("guruterminal-finance")
except PackageNotFoundError:
    WORKER_VERSION = "1.0.0"

DEFAULT_TIMEOUT_MS = 30_000
MAX_TIMEOUT_MS = 300_000
MAX_SOURCES = 128


def _require_mapping(value: object, path: str) -> Mapping[str, Any]:
    if not isinstance(value, Mapping):
        raise invalid_params(f"{path} must be an object", path=path)
    return value


def _reject_unknown(value: Mapping[str, Any], allowed: set[str], path: str) -> None:
    unknown = sorted(set(value) - allowed)
    if unknown:
        raise invalid_params(
            f"{path} contains unsupported fields: {', '.join(unknown)}", path=path
        )


def _required_text(
    value: Mapping[str, Any], key: str, path: str, *, maximum: int = 256
) -> str:
    item = value.get(key)
    item_path = f"{path}.{key}"
    if not isinstance(item, str) or not item.strip():
        raise invalid_params(f"{item_path} must be a non-empty string", path=item_path)
    if len(item) > maximum:
        raise invalid_params(
            f"{item_path} must be at most {maximum} characters", path=item_path
        )
    return item


def _optional_text(
    value: Mapping[str, Any], key: str, path: str, *, maximum: int
) -> str | None:
    item = value.get(key)
    if item is None:
        return None
    item_path = f"{path}.{key}"
    if not isinstance(item, str) or not item.strip():
        raise invalid_params(f"{item_path} must be a non-empty string", path=item_path)
    if len(item) > maximum:
        raise invalid_params(
            f"{item_path} must be at most {maximum} characters", path=item_path
        )
    return item


def parse_aware_datetime(value: object, path: str) -> datetime:
    if not isinstance(value, str) or not value:
        raise invalid_context(f"{path} must be an ISO-8601 timestamp", path=path)
    candidate = value[:-1] + "+00:00" if value.endswith("Z") else value
    try:
        parsed = datetime.fromisoformat(candidate)
    except ValueError as error:
        raise invalid_context(
            f"{path} must be a valid ISO-8601 timestamp", path=path
        ) from error
    if parsed.tzinfo is None or parsed.utcoffset() is None:
        raise invalid_context(f"{path} must include a timezone", path=path)
    return parsed.astimezone(timezone.utc)


def format_datetime(value: datetime) -> str:
    return value.astimezone(timezone.utc).isoformat().replace("+00:00", "Z")


@dataclass(frozen=True, slots=True)
class ProvenanceSource:
    source_id: str
    provider: str
    available_at: datetime
    retrieved_at: datetime
    as_of: datetime | None = None
    uri: str | None = None

    @classmethod
    def from_json(cls, value: object, path: str) -> "ProvenanceSource":
        source = _require_mapping(value, path)
        _reject_unknown(
            source,
            {
                "source_id",
                "provider",
                "available_at",
                "retrieved_at",
                "as_of",
                "uri",
            },
            path,
        )
        source_id = _required_text(source, "source_id", path, maximum=128)
        provider = _required_text(source, "provider", path, maximum=128)
        available_at = parse_aware_datetime(
            source.get("available_at"), f"{path}.available_at"
        )
        retrieved_at = parse_aware_datetime(
            source.get("retrieved_at"), f"{path}.retrieved_at"
        )
        if retrieved_at < available_at:
            raise invalid_context(
                f"{path}.retrieved_at cannot precede available_at",
                path=f"{path}.retrieved_at",
            )
        as_of_value = source.get("as_of")
        as_of = (
            parse_aware_datetime(as_of_value, f"{path}.as_of")
            if as_of_value is not None
            else None
        )
        if as_of is not None and as_of > available_at:
            raise invalid_context(
                f"{path}.as_of cannot be later than available_at",
                path=f"{path}.as_of",
            )
        uri = _optional_text(source, "uri", path, maximum=2048)
        return cls(
            source_id=source_id,
            provider=provider,
            available_at=available_at,
            retrieved_at=retrieved_at,
            as_of=as_of,
            uri=uri,
        )

    def to_json(self) -> dict[str, Any]:
        result: dict[str, Any] = {
            "source_id": self.source_id,
            "provider": self.provider,
            "available_at": format_datetime(self.available_at),
            "retrieved_at": format_datetime(self.retrieved_at),
        }
        if self.as_of is not None:
            result["as_of"] = format_datetime(self.as_of)
        if self.uri is not None:
            result["uri"] = self.uri
        return result


@dataclass(frozen=True, slots=True)
class DataContext:
    data_cutoff: datetime
    sources: tuple[ProvenanceSource, ...]
    timeout_ms: int

    @classmethod
    def from_json(
        cls, value: object, *, allow_future_sources: bool = False
    ) -> "DataContext":
        context = _require_mapping(value, "params.context")
        _reject_unknown(
            context, {"data_cutoff", "sources", "timeout_ms"}, "params.context"
        )
        data_cutoff = parse_aware_datetime(
            context.get("data_cutoff"), "params.context.data_cutoff"
        )
        raw_sources = context.get("sources")
        if not isinstance(raw_sources, list) or not raw_sources:
            raise invalid_params(
                "params.context.sources must be a non-empty array",
                path="params.context.sources",
            )
        if len(raw_sources) > MAX_SOURCES:
            raise invalid_params(
                f"params.context.sources cannot exceed {MAX_SOURCES} entries",
                path="params.context.sources",
            )
        sources = tuple(
            ProvenanceSource.from_json(item, f"params.context.sources[{index}]")
            for index, item in enumerate(raw_sources)
        )
        source_ids = [source.source_id for source in sources]
        if len(set(source_ids)) != len(source_ids):
            raise invalid_params(
                "params.context.sources contains duplicate source_id values",
                path="params.context.sources",
            )
        if not allow_future_sources:
            for index, source in enumerate(sources):
                if source.available_at > data_cutoff:
                    raise invalid_context(
                        "Source was not available at the data cutoff",
                        path=f"params.context.sources[{index}].available_at",
                    )
        timeout_ms = context.get("timeout_ms", DEFAULT_TIMEOUT_MS)
        if (
            isinstance(timeout_ms, bool)
            or not isinstance(timeout_ms, int)
            or not 1 <= timeout_ms <= MAX_TIMEOUT_MS
        ):
            raise invalid_params(
                f"params.context.timeout_ms must be an integer from 1 to {MAX_TIMEOUT_MS}",
                path="params.context.timeout_ms",
            )
        return cls(
            data_cutoff=data_cutoff,
            sources=sources,
            timeout_ms=timeout_ms,
        )

    def source_map(self) -> dict[str, ProvenanceSource]:
        return {source.source_id: source for source in self.sources}

    def to_json(self) -> dict[str, Any]:
        return {
            "data_cutoff": format_datetime(self.data_cutoff),
            "timeout_ms": self.timeout_ms,
            "sources": [source.to_json() for source in self.sources],
        }


def canonical_sha256(value: object) -> str:
    encoded = json.dumps(
        value,
        ensure_ascii=False,
        allow_nan=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()
