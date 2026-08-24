"""Shared execution and validation contracts for finance tools."""

from __future__ import annotations

import threading
import time
from dataclasses import dataclass
from decimal import Decimal, InvalidOperation, localcontext
from typing import Any, Callable, Mapping

from .errors import RequestCancelled, RequestTimedOut, invalid_params
from .schemas import DataContext

MAX_ROWS = 10_000
MAX_DECIMAL_DIGITS = 50
MAX_DECIMAL_EXPONENT = 100

ProgressCallback = Callable[[str, int, int], None]


@dataclass(frozen=True, slots=True)
class ToolSpec:
    name: str
    version: str
    description: str
    input_schema: dict[str, Any]
    allow_future_sources: bool = False

    def to_json(self) -> dict[str, Any]:
        return {
            "name": self.name,
            "version": self.version,
            "description": self.description,
            "input_schema": self.input_schema,
        }


@dataclass(frozen=True, slots=True)
class ToolComputation:
    data: dict[str, Any]
    used_source_ids: frozenset[str]
    warnings: tuple[str, ...] = ()


@dataclass(slots=True)
class ExecutionControl:
    cancel_event: threading.Event
    deadline: float
    progress_callback: ProgressCallback

    def checkpoint(self) -> None:
        if self.cancel_event.is_set():
            raise RequestCancelled()
        if time.monotonic() >= self.deadline:
            raise RequestTimedOut()

    def progress(self, stage: str, completed: int, total: int) -> None:
        self.checkpoint()
        self.progress_callback(stage, completed, total)


def _require_arguments(value: object) -> Mapping[str, Any]:
    if not isinstance(value, Mapping):
        raise invalid_params(
            "params.arguments must be an object", path="params.arguments"
        )
    return value


def _reject_unknown(arguments: Mapping[str, Any], allowed: set[str]) -> None:
    unknown = sorted(set(arguments) - allowed)
    if unknown:
        raise invalid_params(
            f"params.arguments contains unsupported fields: {', '.join(unknown)}",
            path="params.arguments",
        )


def _decimal(value: object, path: str) -> Decimal:
    if isinstance(value, bool) or not isinstance(value, (str, int, float, Decimal)):
        raise invalid_params(f"{path} must be a decimal string or number", path=path)
    if isinstance(value, str) and not value.strip():
        raise invalid_params(f"{path} cannot be empty", path=path)
    try:
        parsed = Decimal(str(value))
    except InvalidOperation as error:
        raise invalid_params(f"{path} must be a valid decimal", path=path) from error
    if not parsed.is_finite():
        raise invalid_params(f"{path} must be finite", path=path)
    decimal_tuple = parsed.as_tuple()
    if len(decimal_tuple.digits) > MAX_DECIMAL_DIGITS:
        raise invalid_params(
            f"{path} cannot exceed {MAX_DECIMAL_DIGITS} significant digits", path=path
        )
    if abs(decimal_tuple.exponent) > MAX_DECIMAL_EXPONENT:
        raise invalid_params(
            f"{path} exponent cannot exceed {MAX_DECIMAL_EXPONENT}", path=path
        )
    return parsed


def _precision(arguments: Mapping[str, Any]) -> int:
    value = arguments.get("precision", 6)
    if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= 12:
        raise invalid_params(
            "params.arguments.precision must be an integer from 0 to 12",
            path="params.arguments.precision",
        )
    return value


def _rounded_text(value: Decimal, precision: int) -> str:
    quantum = Decimal(1).scaleb(-precision)
    try:
        with localcontext() as context:
            context.prec = 80
            rounded = value.quantize(quantum)
    except InvalidOperation as error:
        raise invalid_params("Calculated value is outside supported range") from error
    text = format(rounded, "f")
    if "." in text:
        text = text.rstrip("0").rstrip(".")
    return "0" if text in {"-0", ""} else text


def _all_source_ids(context: DataContext) -> frozenset[str]:
    return frozenset(source.source_id for source in context.sources)
