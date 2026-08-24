from __future__ import annotations

from decimal import Decimal
import threading
import time

import pytest

from conftest import context, source
from guruterminal_finance.errors import RequestCancelled, RequestTimedOut, WorkerError
from guruterminal_finance.tool_runtime import ExecutionControl
from guruterminal_finance.tools import execute_tool


def run_tool(name: str, arguments: object, context_value: object) -> dict[str, object]:
    return execute_tool(
        name,
        arguments,
        context_value,
        cancel_event=threading.Event(),
        progress_callback=lambda _stage, _completed, _total: None,
    )


def test_percentage_change_rejects_unknown_unit_and_grouped_decimals() -> None:
    with pytest.raises(WorkerError, match="unsupported fields: unit"):
        run_tool(
            "percentage_change",
            {"start": "80", "end": "100", "unit": "percent"},
            context(),
        )
    with pytest.raises(WorkerError, match="start must be a valid decimal"):
        run_tool(
            "percentage_change",
            {"start": "69,154", "end": "100"},
            context(),
        )


def test_percentage_change_is_decimal_and_provenanced() -> None:
    result = run_tool(
        "percentage_change",
        {"start": "80", "end": "100", "precision": 2},
        context(),
    )

    assert result["data"] == {
        "kind": "scalar",
        "value": "25",
        "unit": "percent",
        "formula": "((end - start) / start) * 100",
        "inputs": {"start": "80", "end": "100"},
    }
    provenance = result["provenance"]
    assert isinstance(provenance, dict)
    assert provenance["data_cutoff"] == "2025-01-01T00:00:00Z"
    assert len(provenance["input_sha256"]) == 64
    assert [item["source_id"] for item in provenance["sources"]] == ["source-1"]


def test_ratio_uses_multiplier_and_rejects_zero_denominator() -> None:
    result = run_tool(
        "ratio",
        {
            "numerator": "5",
            "denominator": "2",
            "multiplier": "100",
            "unit": "percent",
            "precision": 4,
        },
        context(),
    )
    assert result["data"]["value"] == "250"

    with pytest.raises(WorkerError, match="denominator cannot be zero"):
        run_tool(
            "ratio",
            {"numerator": "5", "denominator": "0"},
            context(),
        )


def test_cagr_uses_period_count_and_decimal_provenance() -> None:
    result = run_tool(
        "compound_annual_growth_rate",
        {"start": "100", "end": "121", "periods": 2, "precision": 4},
        context(),
    )

    assert result["data"] == {
        "kind": "scalar",
        "value": "10",
        "unit": "percent_per_period",
        "formula": "((end / start) ** (1 / periods) - 1) * 100",
        "inputs": {"start": "100", "end": "121", "periods": 2},
    }
    assert result["provenance"]["sources"][0]["source_id"] == "source-1"


def test_discounted_cash_flow_bridges_enterprise_equity_and_per_share_value() -> None:
    result = run_tool(
        "discounted_cash_flow",
        {
            "cash_flows": ["100", "110"],
            "discount_rate": "0.10",
            "terminal_growth_rate": "0.02",
            "net_debt": "50",
            "shares_outstanding": "10",
            "currency": "USD",
            "precision": 4,
        },
        context(),
    )

    data = result["data"]
    assert data["kind"] == "valuation"
    assert data["currency"] == "USD"
    assert data["cash_flow_present_values"] == ["90.9091", "90.9091"]
    assert data["terminal_value"] == "1402.5"
    assert data["terminal_present_value"] == "1159.0909"
    assert data["enterprise_value"] == "1340.9091"
    assert data["equity_value"] == "1290.9091"
    assert data["per_share_value"] == "129.0909"
    assert result["warnings"] == [
        "terminal_value_exceeds_80_percent_of_enterprise_value"
    ]


def test_discounted_cash_flow_rejects_invalid_terminal_and_share_assumptions() -> None:
    with pytest.raises(WorkerError, match="terminal_growth_rate < discount_rate"):
        run_tool(
            "discounted_cash_flow",
            {
                "cash_flows": ["100"],
                "discount_rate": "0.08",
                "terminal_growth_rate": "0.08",
                "currency": "USD",
            },
            context(),
        )

    with pytest.raises(WorkerError, match="shares_outstanding must be greater"):
        run_tool(
            "discounted_cash_flow",
            {
                "cash_flows": ["100"],
                "discount_rate": "0.08",
                "terminal_value": "500",
                "shares_outstanding": "0",
                "currency": "USD",
            },
            context(),
        )


def test_future_source_is_rejected_for_derived_calculation() -> None:
    with pytest.raises(WorkerError) as captured:
        run_tool(
            "percentage_change",
            {"start": "80", "end": "100"},
            context(
                source(
                    available_at="2025-02-01T00:00:00Z",
                    retrieved_at="2025-02-02T00:00:00Z",
                )
            ),
        )
    assert captured.value.code == -32010


def test_point_in_time_filter_never_returns_future_rows_or_sources() -> None:
    old_source = source("old")
    future_source = source(
        "future",
        available_at="2025-02-01T00:00:00Z",
        retrieved_at="2025-02-02T00:00:00Z",
    )
    result = run_tool(
        "point_in_time_filter",
        {
            "rows": [
                {
                    "source_id": "old",
                    "available_at": "2024-12-01T00:00:00Z",
                    "value": 10,
                },
                {
                    "source_id": "future",
                    "available_at": "2025-02-01T00:00:00Z",
                    "value": 99,
                },
            ]
        },
        context(old_source, future_source),
    )

    assert result["data"]["rows"] == [
        {
            "source_id": "old",
            "available_at": "2024-12-01T00:00:00Z",
            "value": 10,
        }
    ]
    assert result["data"]["excluded_count"] == 1
    provenance = result["provenance"]
    assert [item["source_id"] for item in provenance["sources"]] == ["old"]


def test_point_in_time_filter_rejects_backdated_row_from_future_source() -> None:
    future_source = source(
        "future",
        available_at="2025-02-01T00:00:00Z",
        retrieved_at="2025-02-02T00:00:00Z",
    )
    with pytest.raises(WorkerError) as captured:
        run_tool(
            "point_in_time_filter",
            {
                "rows": [
                    {
                        "source_id": "future",
                        "available_at": "2024-12-01T00:00:00Z",
                        "value": 99,
                    }
                ]
            },
            context(future_source),
        )
    assert captured.value.code == -32010


def test_series_statistics_and_risk_metrics_are_decimal() -> None:
    stats = run_tool(
        "series_statistics",
        {"values": ["100", "110", "130"], "periods_per_year": 1, "precision": 4},
        context(),
    )
    assert stats["data"]["cumulative_return"] == "0.3"
    assert Decimal(stats["data"]["annualized_return"]) > 0
    assert stats["data"]["max_drawdown"] == "0"

    risk = run_tool(
        "risk_metrics",
        {
            "values": ["100", "110", "130"],
            "market_values": ["200", "220", "260"],
            "precision": 4,
        },
        context(),
    )
    assert risk["data"]["kind"] == "risk_metrics"
    assert Decimal(risk["data"]["correlation"]) > Decimal("0.99")
    assert Decimal(risk["data"]["beta"]) > 0


def test_wacc_ev_bridge_currency_and_period_aggregate_reject_missing_inputs() -> None:
    wacc = run_tool(
        "weighted_average_cost_of_capital",
        {
            "cost_of_equity": "0.10",
            "cost_of_debt": "0.05",
            "equity_weight": "0.6",
            "debt_weight": "0.4",
            "tax_rate": "0.2",
            "precision": 4,
        },
        context(),
    )
    assert wacc["data"]["value"] == "0.076"

    bridge = run_tool(
        "enterprise_value_bridge",
        {
            "enterprise_value": "1000",
            "net_debt": "200",
            "currency": "USD",
            "precision": 2,
        },
        context(),
    )
    assert bridge["data"]["equity_value"] == "800"

    converted = run_tool(
        "currency_convert",
        {
            "amount": "100",
            "currency": "USD",
            "quote_currency": "KRW",
            "fx_rate": "1300",
            "fx_as_of": "2024-06-30",
            "precision": 0,
        },
        context(),
    )
    assert converted["data"]["value"] == "130000"
    assert converted["data"]["inputs"]["fx_as_of"] == "2024-06-30"

    trailing = run_tool(
        "period_aggregate",
        {
            "values": ["10", "20", "30", "40"],
            "dates": ["2023-03-31", "2023-06-30", "2023-09-30", "2023-12-31"],
            "periods": 4,
            "precision": 0,
        },
        context(),
    )
    assert trailing["data"]["value"] == "100"

    with pytest.raises(WorkerError, match="fx_rate"):
        run_tool(
            "currency_convert",
            {"amount": "100", "currency": "USD", "quote_currency": "KRW"},
            context(),
        )
    with pytest.raises(WorkerError, match="unique and ascending"):
        run_tool(
            "period_aggregate",
            {
                "values": ["10", "20"],
                "dates": ["2023-06-30", "2023-06-30"],
            },
            context(),
        )


def test_irr_and_dcf_sensitivity_are_deterministic() -> None:
    irr = run_tool(
        "internal_rate_of_return",
        {"cash_flows": ["-100", "60", "60"], "precision": 4},
        context(),
    )
    assert irr["data"]["method"] == "irr"
    assert Decimal(irr["data"]["value"]) > 0

    grid = run_tool(
        "dcf_sensitivity",
        {
            "cash_flows": ["100"],
            "discount_rate": "0.10",
            "terminal_growth_rate": "0.02",
            "currency": "USD",
            "discount_rate_shocks": ["0", "0.01"],
            "growth_rate_shocks": ["0"],
            "precision": 2,
        },
        context(),
    )
    assert grid["data"]["kind"] == "sensitivity_grid"
    assert len(grid["data"]["cells"]) == 2


@pytest.mark.parametrize(
    "name", ["python.eval", "script.run", "package.install", "shell.execute"]
)
def test_arbitrary_execution_surfaces_are_not_tools(name: str) -> None:
    with pytest.raises(WorkerError, match="Unknown or forbidden tool"):
        run_tool(name, {}, context())


def test_execution_control_supports_cancel_and_timeout() -> None:
    cancelled = threading.Event()
    cancelled.set()
    with pytest.raises(RequestCancelled):
        ExecutionControl(cancelled, time.monotonic() + 10, lambda *_: None).checkpoint()

    with pytest.raises(RequestTimedOut):
        ExecutionControl(
            threading.Event(), time.monotonic() - 1, lambda *_: None
        ).checkpoint()
