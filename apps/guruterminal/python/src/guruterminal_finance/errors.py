"""Stable worker errors exposed through JSON-RPC."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any


@dataclass(slots=True)
class WorkerError(Exception):
    code: int
    message: str
    data: dict[str, Any] | None = None

    def __str__(self) -> str:
        return self.message


def invalid_params(message: str, *, path: str | None = None) -> WorkerError:
    data = {"kind": "validation_error"}
    if path is not None:
        data["path"] = path
    return WorkerError(-32602, message, data)


def invalid_context(message: str, *, path: str | None = None) -> WorkerError:
    data = {"kind": "invalid_context"}
    if path is not None:
        data["path"] = path
    return WorkerError(-32010, message, data)


def provider_error(message: str) -> WorkerError:
    return WorkerError(-32020, message, {"kind": "provider_error"})


class RequestCancelled(WorkerError):
    def __init__(self) -> None:
        super().__init__(-32800, "Request cancelled", {"kind": "cancelled"})


class RequestTimedOut(WorkerError):
    def __init__(self) -> None:
        super().__init__(-32001, "Request timed out", {"kind": "timeout"})
