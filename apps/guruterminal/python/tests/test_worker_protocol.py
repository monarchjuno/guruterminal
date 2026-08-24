from __future__ import annotations

import io
import hashlib
import json
import threading
import tomllib
from pathlib import Path
from typing import Any

from conftest import context
from guruterminal_finance.errors import RequestCancelled
from guruterminal_finance import worker as worker_module
from guruterminal_finance.worker import WorkerServer
from guruterminal_finance.schemas import WORKER_VERSION


def request(request_id: str, method: str, params: dict[str, Any]) -> str:
    return json.dumps(
        {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
    )


def run_server(lines: list[str]) -> list[dict[str, Any]]:
    stdin = io.StringIO("\n".join(lines) + "\n")
    stdout = io.StringIO()
    assert WorkerServer(stdin, stdout).serve() == 0
    return [json.loads(line) for line in stdout.getvalue().splitlines()]


def handshake(request_id: str = "hello") -> str:
    return request(
        request_id,
        "system.handshake",
        {
            "protocol_version": "1",
            "client": {"name": "test-client", "version": "0.1.0"},
        },
    )


def response(messages: list[dict[str, Any]], request_id: str) -> dict[str, Any]:
    return next(message for message in messages if message.get("id") == request_id)


def test_ready_and_handshake_are_required() -> None:
    messages = run_server([request("list", "tools.list", {})])
    assert response(messages, "list")["error"]["code"] == -32002


def test_handshake_lists_only_closed_tool_registry() -> None:
    messages = run_server([handshake(), request("list", "tools.list", {})])
    hello = response(messages, "hello")["result"]
    assert hello["protocol_version"] == "1"
    assert hello["python_version"]
    assert len(hello["lock_digest"]) == 64
    assert set(hello["tools"]) == {
        "compound_annual_growth_rate",
        "currency_convert",
        "dcf_sensitivity",
        "discounted_cash_flow",
        "enterprise_value_bridge",
        "internal_rate_of_return",
        "percentage_change",
        "period_aggregate",
        "point_in_time_filter",
        "ratio",
        "risk_metrics",
        "series_statistics",
        "weighted_average_cost_of_capital",
    }
    assert hello["capabilities"]["arbitrary_code"] is False
    assert hello["capabilities"]["http_server"] is False
    names = {tool["name"] for tool in response(messages, "list")["result"]["tools"]}
    assert names == {
        "compound_annual_growth_rate",
        "currency_convert",
        "dcf_sensitivity",
        "discounted_cash_flow",
        "enterprise_value_bridge",
        "internal_rate_of_return",
        "percentage_change",
        "period_aggregate",
        "point_in_time_filter",
        "ratio",
        "risk_metrics",
        "series_statistics",
        "weighted_average_cost_of_capital",
    }


def test_worker_identity_matches_current_project_and_lock() -> None:
    project_root = Path(__file__).resolve().parents[1]
    pyproject = tomllib.loads(
        (project_root / "pyproject.toml").read_text(encoding="utf-8")
    )
    lock_digest = hashlib.sha256((project_root / "uv.lock").read_bytes()).hexdigest()

    assert WORKER_VERSION == pyproject["project"]["version"] == "1.0.0"
    assert worker_module._lock_digest() == lock_digest
    assert lock_digest == (
        "172ddf32098550f75ccd271220268694dc52766d60e9b7deb8ab88310e6605bf"
    )


def test_handshake_accepts_empty_params_for_native_adapter() -> None:
    messages = run_server([request("hello", "system.handshake", {})])
    assert response(messages, "hello")["result"]["protocol_version"] == "1"


def test_tool_call_emits_progress_and_correlated_result() -> None:
    messages = run_server(
        [
            handshake(),
            request(
                "calc",
                "tools.call",
                {
                    "name": "percentage_change",
                    "arguments": {"start": "80", "end": "100"},
                    "context": context(),
                },
            ),
        ]
    )
    progress = [
        message
        for message in messages
        if message.get("method") == "progress" and message["params"]["id"] == "calc"
    ]
    assert progress
    assert response(messages, "calc")["result"]["data"]["value"] == "25"


def test_protocol_rejects_mismatch_nonfinite_json_and_forbidden_tool() -> None:
    mismatch = request(
        "bad-version",
        "system.handshake",
        {
            "protocol_version": "99",
            "client": {"name": "test-client", "version": "0.1.0"},
        },
    )
    messages = run_server(
        [mismatch, '{"jsonrpc":"2.0","id":"bad","method":"x","params":{"x":NaN}}']
    )
    assert response(messages, "bad-version")["error"]["code"] == -32003
    assert any(message.get("error", {}).get("code") == -32700 for message in messages)

    messages = run_server(
        [
            handshake(),
            request(
                "forbidden",
                "tools.call",
                {
                    "name": "python.eval",
                    "arguments": {"code": "2 + 2"},
                    "context": context(),
                },
            ),
        ]
    )
    assert response(messages, "forbidden")["error"]["code"] == -32602


def test_cancel_request_reports_whether_target_was_active() -> None:
    messages = run_server(
        [
            handshake(),
            request("cancel", "system.cancel", {"request_id": "missing"}),
        ]
    )
    assert response(messages, "cancel")["result"] == {
        "request_id": "missing",
        "accepted": False,
    }


def test_cancel_request_interrupts_an_active_tool_call(monkeypatch: Any) -> None:
    def blocking_tool(
        _name: object,
        _arguments: object,
        _context: object,
        *,
        cancel_event: threading.Event,
        progress_callback: Any,
    ) -> dict[str, object]:
        progress_callback("waiting", 0, 1)
        if not cancel_event.wait(timeout=1):
            return {"unexpected": "request was not cancelled"}
        raise RequestCancelled()

    monkeypatch.setattr(worker_module, "execute_tool", blocking_tool)
    messages = run_server(
        [
            handshake(),
            request(
                "calc",
                "tools.call",
                {
                    "name": "ratio",
                    "arguments": {"numerator": "1", "denominator": "1"},
                    "context": context(),
                },
            ),
            request("cancel", "system.cancel", {"request_id": "calc"}),
        ]
    )

    assert response(messages, "cancel")["result"] == {
        "request_id": "calc",
        "accepted": True,
    }
    assert response(messages, "calc")["error"] == {
        "code": -32800,
        "message": "Request cancelled",
        "data": {"kind": "cancelled"},
    }


def test_shutdown_stops_before_later_input() -> None:
    messages = run_server(
        [
            handshake(),
            request("bye", "system.shutdown", {}),
            request("ignored", "tools.list", {}),
        ]
    )
    assert response(messages, "bye")["result"] == {"stopping": True}
    assert not any(message.get("id") == "ignored" for message in messages)
