"""Deterministic, provider-independent finance calculations."""

from __future__ import annotations

from datetime import date
from decimal import Decimal, localcontext
import re
from typing import Any

from .errors import invalid_params, invalid_context
from .schemas import DataContext, format_datetime, parse_aware_datetime
from .tool_runtime import (
    MAX_ROWS,
    ExecutionControl,
    ToolComputation,
    ToolSpec,
    _all_source_ids,
    _decimal,
    _precision,
    _reject_unknown,
    _require_arguments,
    _rounded_text,
)

PERCENTAGE_CHANGE_SPEC = ToolSpec(
    name="percentage_change",
    version="1.0.0",
    description="Compute ((end - start) / start) * 100 with decimal arithmetic.",
    input_schema={
        "type": "object",
        "additionalProperties": False,
        "required": ["start", "end"],
        "properties": {
            "start": {"type": ["string", "number"]},
            "end": {"type": ["string", "number"]},
            "precision": {"type": "integer", "minimum": 0, "maximum": 12},
        },
    },
)

RATIO_SPEC = ToolSpec(
    name="ratio",
    version="1.0.0",
    description="Compute numerator / denominator * multiplier with decimal arithmetic.",
    input_schema={
        "type": "object",
        "additionalProperties": False,
        "required": ["numerator", "denominator"],
        "properties": {
            "numerator": {"type": ["string", "number"]},
            "denominator": {"type": ["string", "number"]},
            "multiplier": {"type": ["string", "number"], "default": "1"},
            "unit": {"type": "string", "maxLength": 32},
            "precision": {"type": "integer", "minimum": 0, "maximum": 12},
        },
    },
)

CAGR_SPEC = ToolSpec(
    name="compound_annual_growth_rate",
    version="1.0.0",
    description="Compute ((end / start) ** (1 / periods) - 1) * 100 with decimal arithmetic.",
    input_schema={
        "type": "object",
        "additionalProperties": False,
        "required": ["start", "end", "periods"],
        "properties": {
            "start": {"type": ["string", "number"]},
            "end": {"type": ["string", "number"]},
            "periods": {"type": "integer", "minimum": 1, "maximum": 100},
            "precision": {"type": "integer", "minimum": 0, "maximum": 12},
        },
    },
)

DISCOUNTED_CASH_FLOW_SPEC = ToolSpec(
    name="discounted_cash_flow",
    version="1.0.0",
    description=(
        "Discount annual end-of-period cash flows and an optional terminal value, "
        "then bridge enterprise value to equity and per-share value."
    ),
    input_schema={
        "type": "object",
        "additionalProperties": False,
        "required": ["cash_flows", "discount_rate", "currency"],
        "properties": {
            "cash_flows": {
                "type": "array",
                "minItems": 1,
                "maxItems": 30,
                "items": {"type": ["string", "number"]},
            },
            "discount_rate": {"type": ["string", "number"]},
            "terminal_growth_rate": {"type": ["string", "number"]},
            "terminal_value": {"type": ["string", "number"]},
            "net_debt": {"type": ["string", "number"], "default": "0"},
            "shares_outstanding": {"type": ["string", "number"]},
            "currency": {"type": "string", "pattern": "^[A-Z]{3}$"},
            "precision": {"type": "integer", "minimum": 0, "maximum": 12},
        },
    },
)

POINT_IN_TIME_FILTER_SPEC = ToolSpec(
    name="point_in_time_filter",
    version="1.0.0",
    description="Retain only rows whose availability and source precede the data cutoff.",
    input_schema={
        "type": "object",
        "additionalProperties": False,
        "required": ["rows"],
        "properties": {
            "rows": {
                "type": "array",
                "maxItems": MAX_ROWS,
                "items": {
                    "type": "object",
                    "required": ["available_at", "source_id"],
                },
            }
        },
    },
    allow_future_sources=True,
)


def _percentage_change(
    arguments_value: object, context: DataContext, control: ExecutionControl
) -> ToolComputation:
    arguments = _require_arguments(arguments_value)
    _reject_unknown(arguments, {"start", "end", "precision"})
    if "start" not in arguments or "end" not in arguments:
        raise invalid_params(
            "params.arguments requires start and end", path="params.arguments"
        )
    control.progress("validating", 0, 1)
    start = _decimal(arguments["start"], "params.arguments.start")
    end = _decimal(arguments["end"], "params.arguments.end")
    if start == 0:
        raise invalid_params(
            "params.arguments.start cannot be zero",
            path="params.arguments.start",
        )
    precision = _precision(arguments)
    control.checkpoint()
    with localcontext() as decimal_context:
        decimal_context.prec = 80
        value = ((end - start) / start) * Decimal(100)
    control.progress("computed", 1, 1)
    return ToolComputation(
        data={
            "kind": "scalar",
            "value": _rounded_text(value, precision),
            "unit": "percent",
            "formula": "((end - start) / start) * 100",
            "inputs": {"start": str(start), "end": str(end)},
        },
        used_source_ids=_all_source_ids(context),
    )


def _ratio(
    arguments_value: object, context: DataContext, control: ExecutionControl
) -> ToolComputation:
    arguments = _require_arguments(arguments_value)
    _reject_unknown(
        arguments, {"numerator", "denominator", "multiplier", "unit", "precision"}
    )
    if "numerator" not in arguments or "denominator" not in arguments:
        raise invalid_params(
            "params.arguments requires numerator and denominator",
            path="params.arguments",
        )
    control.progress("validating", 0, 1)
    numerator = _decimal(arguments["numerator"], "params.arguments.numerator")
    denominator = _decimal(arguments["denominator"], "params.arguments.denominator")
    multiplier = _decimal(
        arguments.get("multiplier", "1"), "params.arguments.multiplier"
    )
    if denominator == 0:
        raise invalid_params(
            "params.arguments.denominator cannot be zero",
            path="params.arguments.denominator",
        )
    unit = arguments.get("unit", "ratio")
    if not isinstance(unit, str) or not unit.strip() or len(unit) > 32:
        raise invalid_params(
            "params.arguments.unit must be a non-empty string of at most 32 characters",
            path="params.arguments.unit",
        )
    precision = _precision(arguments)
    control.checkpoint()
    with localcontext() as decimal_context:
        decimal_context.prec = 80
        value = (numerator / denominator) * multiplier
    control.progress("computed", 1, 1)
    return ToolComputation(
        data={
            "kind": "scalar",
            "value": _rounded_text(value, precision),
            "unit": unit,
            "formula": "numerator / denominator * multiplier",
            "inputs": {
                "numerator": str(numerator),
                "denominator": str(denominator),
                "multiplier": str(multiplier),
            },
        },
        used_source_ids=_all_source_ids(context),
    )


def _compound_annual_growth_rate(
    arguments_value: object, context: DataContext, control: ExecutionControl
) -> ToolComputation:
    arguments = _require_arguments(arguments_value)
    _reject_unknown(arguments, {"start", "end", "periods", "precision"})
    if not {"start", "end", "periods"}.issubset(arguments):
        raise invalid_params(
            "params.arguments requires start, end, and periods",
            path="params.arguments",
        )
    control.progress("validating", 0, 1)
    start = _decimal(arguments["start"], "params.arguments.start")
    end = _decimal(arguments["end"], "params.arguments.end")
    periods = arguments["periods"]
    if start <= 0 or end < 0:
        raise invalid_params(
            "params.arguments requires start > 0 and end >= 0",
            path="params.arguments",
        )
    if (
        isinstance(periods, bool)
        or not isinstance(periods, int)
        or not 1 <= periods <= 100
    ):
        raise invalid_params(
            "params.arguments.periods must be an integer from 1 to 100",
            path="params.arguments.periods",
        )
    precision = _precision(arguments)
    control.checkpoint()
    with localcontext() as decimal_context:
        decimal_context.prec = 80
        value = (
            (end / start) ** (Decimal(1) / Decimal(periods)) - Decimal(1)
        ) * Decimal(100)
    control.progress("computed", 1, 1)
    return ToolComputation(
        data={
            "kind": "scalar",
            "value": _rounded_text(value, precision),
            "unit": "percent_per_period",
            "formula": "((end / start) ** (1 / periods) - 1) * 100",
            "inputs": {"start": str(start), "end": str(end), "periods": periods},
        },
        used_source_ids=_all_source_ids(context),
    )


def _discounted_cash_flow(
    arguments_value: object, context: DataContext, control: ExecutionControl
) -> ToolComputation:
    arguments = _require_arguments(arguments_value)
    allowed = {
        "cash_flows",
        "discount_rate",
        "terminal_growth_rate",
        "terminal_value",
        "net_debt",
        "shares_outstanding",
        "currency",
        "precision",
    }
    _reject_unknown(arguments, allowed)
    if not {"cash_flows", "discount_rate", "currency"}.issubset(arguments):
        raise invalid_params(
            "params.arguments requires cash_flows, discount_rate, and currency",
            path="params.arguments",
        )
    raw_cash_flows = arguments["cash_flows"]
    if not isinstance(raw_cash_flows, list) or not 1 <= len(raw_cash_flows) <= 30:
        raise invalid_params(
            "params.arguments.cash_flows must contain 1 to 30 annual values",
            path="params.arguments.cash_flows",
        )
    cash_flows = [
        _decimal(value, f"params.arguments.cash_flows[{index}]")
        for index, value in enumerate(raw_cash_flows)
    ]
    discount_rate = _decimal(
        arguments["discount_rate"], "params.arguments.discount_rate"
    )
    if discount_rate <= Decimal("-1"):
        raise invalid_params(
            "params.arguments.discount_rate must be greater than -1",
            path="params.arguments.discount_rate",
        )
    has_terminal_growth = "terminal_growth_rate" in arguments
    has_terminal_value = "terminal_value" in arguments
    if has_terminal_growth and has_terminal_value:
        raise invalid_params(
            "provide terminal_growth_rate or terminal_value, not both",
            path="params.arguments",
        )
    currency = arguments["currency"]
    if not isinstance(currency, str) or not re.fullmatch(r"[A-Z]{3}", currency):
        raise invalid_params(
            "params.arguments.currency must be a three-letter uppercase code",
            path="params.arguments.currency",
        )
    net_debt = _decimal(arguments.get("net_debt", "0"), "params.arguments.net_debt")
    shares = (
        _decimal(arguments["shares_outstanding"], "params.arguments.shares_outstanding")
        if "shares_outstanding" in arguments
        else None
    )
    if shares is not None and shares <= 0:
        raise invalid_params(
            "params.arguments.shares_outstanding must be greater than zero",
            path="params.arguments.shares_outstanding",
        )
    precision = _precision(arguments)
    control.progress("discounting", 0, len(cash_flows) + 1)
    present_values: list[Decimal] = []
    with localcontext() as decimal_context:
        decimal_context.prec = 80
        base = Decimal(1) + discount_rate
        for index, cash_flow in enumerate(cash_flows, start=1):
            present_values.append(cash_flow / (base**index))
            control.progress("discounting", index, len(cash_flows) + 1)

        terminal_growth: Decimal | None = None
        if has_terminal_growth:
            terminal_growth = _decimal(
                arguments["terminal_growth_rate"],
                "params.arguments.terminal_growth_rate",
            )
            if terminal_growth >= discount_rate or cash_flows[-1] <= 0:
                raise invalid_params(
                    "Gordon growth requires terminal_growth_rate < discount_rate and a positive final cash flow",
                    path="params.arguments.terminal_growth_rate",
                )
            terminal_value = (
                cash_flows[-1]
                * (Decimal(1) + terminal_growth)
                / (discount_rate - terminal_growth)
            )
        elif has_terminal_value:
            terminal_value = _decimal(
                arguments["terminal_value"], "params.arguments.terminal_value"
            )
        else:
            terminal_value = Decimal(0)
        terminal_present_value = terminal_value / (base ** len(cash_flows))
        enterprise_value = sum(present_values, Decimal(0)) + terminal_present_value
        equity_value = enterprise_value - net_debt
        per_share_value = equity_value / shares if shares is not None else None

    warnings: list[str] = []
    if enterprise_value > 0 and terminal_present_value > 0:
        terminal_share = terminal_present_value / enterprise_value
        if terminal_share > Decimal("0.8"):
            warnings.append("terminal_value_exceeds_80_percent_of_enterprise_value")
    control.progress("computed", len(cash_flows) + 1, len(cash_flows) + 1)
    data: dict[str, Any] = {
        "kind": "valuation",
        "currency": currency,
        "enterprise_value": _rounded_text(enterprise_value, precision),
        "equity_value": _rounded_text(equity_value, precision),
        "cash_flow_present_values": [
            _rounded_text(value, precision) for value in present_values
        ],
        "terminal_value": _rounded_text(terminal_value, precision),
        "terminal_present_value": _rounded_text(terminal_present_value, precision),
        "formula": "enterprise_value = sum(cash_flow[t] / (1 + discount_rate)^t) + terminal_value / (1 + discount_rate)^n; equity_value = enterprise_value - net_debt",
        "inputs": {
            "cash_flows": [str(value) for value in cash_flows],
            "discount_rate": str(discount_rate),
            "terminal_growth_rate": str(terminal_growth)
            if terminal_growth is not None
            else None,
            "terminal_value_input": str(arguments["terminal_value"])
            if has_terminal_value
            else None,
            "net_debt": str(net_debt),
            "shares_outstanding": str(shares) if shares is not None else None,
        },
    }
    if per_share_value is not None:
        data["per_share_value"] = _rounded_text(per_share_value, precision)
    return ToolComputation(
        data=data,
        used_source_ids=_all_source_ids(context),
        warnings=tuple(warnings),
    )


def _point_in_time_filter(
    arguments_value: object, context: DataContext, control: ExecutionControl
) -> ToolComputation:
    arguments = _require_arguments(arguments_value)
    _reject_unknown(arguments, {"rows"})
    rows = arguments.get("rows")
    if not isinstance(rows, list):
        raise invalid_params(
            "params.arguments.rows must be an array", path="params.arguments.rows"
        )
    if len(rows) > MAX_ROWS:
        raise invalid_params(
            f"params.arguments.rows cannot exceed {MAX_ROWS} entries",
            path="params.arguments.rows",
        )

    sources = context.source_map()
    retained: list[dict[str, Any]] = []
    used_source_ids: set[str] = set()
    total = max(len(rows), 1)
    control.progress("filtering", 0, total)

    for index, raw_row in enumerate(rows):
        if not isinstance(raw_row, dict):
            raise invalid_params(
                f"params.arguments.rows[{index}] must be an object",
                path=f"params.arguments.rows[{index}]",
            )
        row_path = f"params.arguments.rows[{index}]"
        source_id = raw_row.get("source_id")
        if not isinstance(source_id, str) or not source_id:
            raise invalid_params(
                f"{row_path}.source_id must be a non-empty string",
                path=f"{row_path}.source_id",
            )
        source = sources.get(source_id)
        if source is None:
            raise invalid_params(
                f"{row_path}.source_id is not declared in params.context.sources",
                path=f"{row_path}.source_id",
            )
        available_at = parse_aware_datetime(
            raw_row.get("available_at"), f"{row_path}.available_at"
        )
        if available_at <= context.data_cutoff:
            if source.available_at > context.data_cutoff:
                raise invalid_context(
                    "Retained row references a source unavailable at the data cutoff",
                    path=f"{row_path}.source_id",
                )
            retained.append(dict(raw_row))
            used_source_ids.add(source_id)
        if index % 128 == 0 or index + 1 == len(rows):
            control.progress("filtering", index + 1, total)
        else:
            control.checkpoint()

    return ToolComputation(
        data={
            "kind": "rows",
            "rows": retained,
            "row_count": len(retained),
            "excluded_count": len(rows) - len(retained),
            "data_cutoff": format_datetime(context.data_cutoff),
        },
        used_source_ids=frozenset(used_source_ids),
    )


SERIES_STATISTICS_SPEC = ToolSpec(
    name="series_statistics",
    version="1.0.0",
    description="Compute cumulative and annualized return, volatility, and maximum drawdown from a supplied price series.",
    input_schema={
        "type": "object",
        "additionalProperties": False,
        "required": ["values", "periods_per_year"],
        "properties": {
            "values": {
                "type": "array",
                "minItems": 2,
                "maxItems": 6000,
                "items": {"type": ["string", "number"]},
            },
            "dates": {
                "type": "array",
                "minItems": 2,
                "maxItems": 6000,
                "items": {"type": "string"},
            },
            "periods_per_year": {"type": "integer", "minimum": 1, "maximum": 365},
            "precision": {"type": "integer", "minimum": 0, "maximum": 12},
        },
    },
)

INTERNAL_RATE_OF_RETURN_SPEC = ToolSpec(
    name="internal_rate_of_return",
    version="1.0.0",
    description="Solve IRR or XIRR for a cash-flow series. Irregular dates require cash_flow_dates.",
    input_schema={
        "type": "object",
        "additionalProperties": False,
        "required": ["cash_flows"],
        "properties": {
            "cash_flows": {
                "type": "array",
                "minItems": 2,
                "maxItems": 60,
                "items": {"type": ["string", "number"]},
            },
            "cash_flow_dates": {
                "type": "array",
                "minItems": 2,
                "maxItems": 60,
                "items": {"type": "string"},
            },
            "precision": {"type": "integer", "minimum": 0, "maximum": 12},
        },
    },
)

WACC_SPEC = ToolSpec(
    name="weighted_average_cost_of_capital",
    version="1.0.0",
    description="Compute after-tax WACC from supplied costs, weights, and tax rate. Missing inputs are rejected.",
    input_schema={
        "type": "object",
        "additionalProperties": False,
        "required": [
            "cost_of_equity",
            "cost_of_debt",
            "equity_weight",
            "debt_weight",
            "tax_rate",
        ],
        "properties": {
            "cost_of_equity": {"type": ["string", "number"]},
            "cost_of_debt": {"type": ["string", "number"]},
            "equity_weight": {"type": ["string", "number"]},
            "debt_weight": {"type": ["string", "number"]},
            "tax_rate": {"type": ["string", "number"]},
            "precision": {"type": "integer", "minimum": 0, "maximum": 12},
        },
    },
)

ENTERPRISE_VALUE_BRIDGE_SPEC = ToolSpec(
    name="enterprise_value_bridge",
    version="1.0.0",
    description="Bridge enterprise value and equity value. One of enterprise_value or equity_value is required; net_debt is required.",
    input_schema={
        "type": "object",
        "additionalProperties": False,
        "required": ["net_debt", "currency"],
        "properties": {
            "enterprise_value": {"type": ["string", "number"]},
            "equity_value": {"type": ["string", "number"]},
            "net_debt": {"type": ["string", "number"]},
            "minority_interest": {"type": ["string", "number"]},
            "lease_liabilities": {"type": ["string", "number"]},
            "non_operating_assets": {"type": ["string", "number"]},
            "shares_outstanding": {"type": ["string", "number"]},
            "currency": {"type": "string", "pattern": "^[A-Z]{3}$"},
            "precision": {"type": "integer", "minimum": 0, "maximum": 12},
        },
    },
)

PERIOD_AGGREGATE_SPEC = ToolSpec(
    name="period_aggregate",
    version="1.0.0",
    description="Sum a dated series over trailing periods. Duplicate or missing periods are rejected.",
    input_schema={
        "type": "object",
        "additionalProperties": False,
        "required": ["values", "dates"],
        "properties": {
            "values": {
                "type": "array",
                "minItems": 2,
                "maxItems": 120,
                "items": {"type": ["string", "number"]},
            },
            "dates": {
                "type": "array",
                "minItems": 2,
                "maxItems": 120,
                "items": {"type": "string"},
            },
            "periods": {"type": "integer", "minimum": 1, "maximum": 40},
            "precision": {"type": "integer", "minimum": 0, "maximum": 12},
        },
    },
)

CURRENCY_CONVERT_SPEC = ToolSpec(
    name="currency_convert",
    version="1.0.0",
    description="Convert an amount with an explicit FX rate and FX as-of. The rate is never implied.",
    input_schema={
        "type": "object",
        "additionalProperties": False,
        "required": ["amount", "currency", "quote_currency", "fx_rate", "fx_as_of"],
        "properties": {
            "amount": {"type": ["string", "number"]},
            "currency": {"type": "string", "pattern": "^[A-Z]{3}$"},
            "quote_currency": {"type": "string", "pattern": "^[A-Z]{3}$"},
            "fx_rate": {"type": ["string", "number"]},
            "fx_as_of": {"type": "string"},
            "precision": {"type": "integer", "minimum": 0, "maximum": 12},
        },
    },
)

DCF_SENSITIVITY_SPEC = ToolSpec(
    name="dcf_sensitivity",
    version="1.0.0",
    description="Evaluate a DCF on a supplied discount-rate and growth-rate shock grid. Shocks are required.",
    input_schema={
        "type": "object",
        "additionalProperties": False,
        "required": [
            "cash_flows",
            "discount_rate",
            "currency",
            "discount_rate_shocks",
            "growth_rate_shocks",
        ],
        "properties": {
            "cash_flows": {
                "type": "array",
                "minItems": 1,
                "maxItems": 30,
                "items": {"type": ["string", "number"]},
            },
            "discount_rate": {"type": ["string", "number"]},
            "terminal_growth_rate": {"type": ["string", "number"]},
            "terminal_value": {"type": ["string", "number"]},
            "net_debt": {"type": ["string", "number"]},
            "shares_outstanding": {"type": ["string", "number"]},
            "currency": {"type": "string", "pattern": "^[A-Z]{3}$"},
            "discount_rate_shocks": {
                "type": "array",
                "minItems": 1,
                "maxItems": 8,
                "items": {"type": ["string", "number"]},
            },
            "growth_rate_shocks": {
                "type": "array",
                "minItems": 1,
                "maxItems": 8,
                "items": {"type": ["string", "number"]},
            },
            "precision": {"type": "integer", "minimum": 0, "maximum": 12},
        },
    },
)

RISK_METRICS_SPEC = ToolSpec(
    name="risk_metrics",
    version="1.0.0",
    description="Compute correlation and beta between two aligned return or price series.",
    input_schema={
        "type": "object",
        "additionalProperties": False,
        "required": ["values", "market_values"],
        "properties": {
            "values": {
                "type": "array",
                "minItems": 3,
                "maxItems": 6000,
                "items": {"type": ["string", "number"]},
            },
            "market_values": {
                "type": "array",
                "minItems": 3,
                "maxItems": 6000,
                "items": {"type": ["string", "number"]},
            },
            "dates": {
                "type": "array",
                "minItems": 3,
                "maxItems": 6000,
                "items": {"type": "string"},
            },
            "precision": {"type": "integer", "minimum": 0, "maximum": 12},
        },
    },
)


def _require_keys(arguments: dict[str, Any], keys: set[str], path: str) -> None:
    missing = sorted(keys - set(arguments))
    if missing:
        raise invalid_params(
            f"{path} requires {', '.join(missing)}",
            path=path,
        )


def _currency_code(value: object, path: str) -> str:
    if not isinstance(value, str) or not re.fullmatch(r"[A-Z]{3}", value):
        raise invalid_params(
            f"{path} must be a three-letter uppercase code",
            path=path,
        )
    return value


def _parse_date(value: object, path: str) -> date:
    if not isinstance(value, str) or len(value) < 10:
        raise invalid_params(f"{path} must be a YYYY-MM-DD date", path=path)
    try:
        return date.fromisoformat(value[:10])
    except ValueError as error:
        raise invalid_params(f"{path} must be a YYYY-MM-DD date", path=path) from error


def _decimal_series(
    raw: object, path: str, *, minimum: int, maximum: int
) -> list[Decimal]:
    if not isinstance(raw, list) or not minimum <= len(raw) <= maximum:
        raise invalid_params(
            f"{path} must contain {minimum} to {maximum} values",
            path=path,
        )
    return [_decimal(item, f"{path}[{index}]") for index, item in enumerate(raw)]


def _returns(values: list[Decimal], path: str) -> list[Decimal]:
    returns: list[Decimal] = []
    for index in range(1, len(values)):
        previous = values[index - 1]
        if previous == 0:
            raise invalid_params(
                f"{path}[{index - 1}] cannot be zero when computing returns",
                path=f"{path}[{index - 1}]",
            )
        returns.append((values[index] / previous) - Decimal(1))
    return returns


def _population_stdev(values: list[Decimal]) -> Decimal:
    if len(values) < 2:
        raise invalid_params(
            "a series statistic that needs dispersion requires at least two returns",
            path="params.arguments.values",
        )
    mean = sum(values, Decimal(0)) / Decimal(len(values))
    variance = sum((item - mean) ** 2 for item in values) / Decimal(len(values) - 1)
    with localcontext() as decimal_context:
        decimal_context.prec = 80
        return variance.sqrt()


def _series_statistics(
    arguments_value: object, context: DataContext, control: ExecutionControl
) -> ToolComputation:
    arguments = _require_arguments(arguments_value)
    _reject_unknown(arguments, {"values", "dates", "periods_per_year", "precision"})
    _require_keys(arguments, {"values", "periods_per_year"}, "params.arguments")
    values = _decimal_series(
        arguments["values"], "params.arguments.values", minimum=2, maximum=6000
    )
    if any(item <= 0 for item in values):
        raise invalid_params(
            "params.arguments.values must be strictly positive prices",
            path="params.arguments.values",
        )
    periods_per_year = arguments["periods_per_year"]
    if (
        isinstance(periods_per_year, bool)
        or not isinstance(periods_per_year, int)
        or not 1 <= periods_per_year <= 365
    ):
        raise invalid_params(
            "params.arguments.periods_per_year must be an integer from 1 to 365",
            path="params.arguments.periods_per_year",
        )
    dates = arguments.get("dates")
    if dates is not None:
        if not isinstance(dates, list) or len(dates) != len(values):
            raise invalid_params(
                "params.arguments.dates must align 1:1 with values",
                path="params.arguments.dates",
            )
        parsed_dates = [
            _parse_date(item, f"params.arguments.dates[{index}]")
            for index, item in enumerate(dates)
        ]
        if parsed_dates != sorted(parsed_dates) or len(set(parsed_dates)) != len(
            parsed_dates
        ):
            raise invalid_params(
                "params.arguments.dates must be unique and ascending",
                path="params.arguments.dates",
            )
    precision = _precision(arguments)
    control.progress("computing", 0, 1)
    returns = _returns(values, "params.arguments.values")
    with localcontext() as decimal_context:
        decimal_context.prec = 80
        cumulative = (values[-1] / values[0]) - Decimal(1)
        span = Decimal(len(returns))
        annualized = (
            (Decimal(1) + cumulative) ** (Decimal(periods_per_year) / span)
        ) - Decimal(1)
        volatility = _population_stdev(returns) * (Decimal(periods_per_year).sqrt())
        peak = values[0]
        max_drawdown = Decimal(0)
        for price in values:
            if price > peak:
                peak = price
            drawdown = (price / peak) - Decimal(1)
            if drawdown < max_drawdown:
                max_drawdown = drawdown
    control.progress("computed", 1, 1)
    return ToolComputation(
        data={
            "kind": "series_statistics",
            "observation_count": len(values),
            "return_count": len(returns),
            "cumulative_return": _rounded_text(cumulative, precision),
            "annualized_return": _rounded_text(annualized, precision),
            "volatility": _rounded_text(volatility, precision),
            "max_drawdown": _rounded_text(max_drawdown, precision),
            "unit": "decimal",
            "formula": "cumulative = last/first - 1; annualized = (1 + cumulative)^(periods_per_year / n) - 1; volatility = sample_stdev(returns) * sqrt(periods_per_year); max_drawdown = min(price/peak - 1)",
            "inputs": {
                "periods_per_year": periods_per_year,
                "first": str(values[0]),
                "last": str(values[-1]),
            },
        },
        used_source_ids=_all_source_ids(context),
    )


def _npv(cash_flows: list[Decimal], times: list[Decimal], rate: Decimal) -> Decimal:
    base = Decimal(1) + rate
    if base == 0:
        raise invalid_params(
            "discount factor is undefined at rate -1",
            path="params.arguments.cash_flows",
        )
    return sum(
        cash_flow / (base**time)
        for cash_flow, time in zip(cash_flows, times, strict=True)
    )


def _internal_rate_of_return(
    arguments_value: object, context: DataContext, control: ExecutionControl
) -> ToolComputation:
    arguments = _require_arguments(arguments_value)
    _reject_unknown(arguments, {"cash_flows", "cash_flow_dates", "precision"})
    _require_keys(arguments, {"cash_flows"}, "params.arguments")
    cash_flows = _decimal_series(
        arguments["cash_flows"], "params.arguments.cash_flows", minimum=2, maximum=60
    )
    if not (
        any(item > 0 for item in cash_flows) and any(item < 0 for item in cash_flows)
    ):
        raise invalid_params(
            "params.arguments.cash_flows must include both an outflow and an inflow",
            path="params.arguments.cash_flows",
        )
    raw_dates = arguments.get("cash_flow_dates")
    if raw_dates is None:
        times = [Decimal(index) for index in range(len(cash_flows))]
        method = "irr"
    else:
        if not isinstance(raw_dates, list) or len(raw_dates) != len(cash_flows):
            raise invalid_params(
                "params.arguments.cash_flow_dates must align 1:1 with cash_flows",
                path="params.arguments.cash_flow_dates",
            )
        dates = [
            _parse_date(item, f"params.arguments.cash_flow_dates[{index}]")
            for index, item in enumerate(raw_dates)
        ]
        if dates != sorted(dates) or len(set(dates)) != len(dates):
            raise invalid_params(
                "params.arguments.cash_flow_dates must be unique and ascending",
                path="params.arguments.cash_flow_dates",
            )
        origin = dates[0]
        times = [Decimal((item - origin).days) / Decimal(365) for item in dates]
        method = "xirr"
    precision = _precision(arguments)
    control.progress("solving", 0, 1)
    rate = Decimal("0.1")
    with localcontext() as decimal_context:
        decimal_context.prec = 80
        for _ in range(80):
            value = _npv(cash_flows, times, rate)
            bumped = _npv(cash_flows, times, rate + Decimal("0.000001"))
            derivative = (bumped - value) / Decimal("0.000001")
            if derivative == 0:
                break
            nxt = rate - (value / derivative)
            if abs(nxt - rate) < Decimal("0.0000000001"):
                rate = nxt
                break
            rate = nxt
        if abs(_npv(cash_flows, times, rate)) > Decimal("0.0001"):
            raise invalid_params(
                "internal rate of return did not converge",
                path="params.arguments.cash_flows",
            )
    control.progress("computed", 1, 1)
    return ToolComputation(
        data={
            "kind": "rate",
            "method": method,
            "value": _rounded_text(rate, precision),
            "unit": "decimal",
            "formula": "solve sum(cash_flow[t] / (1 + rate)^t) = 0",
            "inputs": {
                "cash_flows": [str(item) for item in cash_flows],
                "cash_flow_dates": raw_dates if isinstance(raw_dates, list) else None,
            },
        },
        used_source_ids=_all_source_ids(context),
    )


def _weighted_average_cost_of_capital(
    arguments_value: object, context: DataContext, control: ExecutionControl
) -> ToolComputation:
    arguments = _require_arguments(arguments_value)
    required = {
        "cost_of_equity",
        "cost_of_debt",
        "equity_weight",
        "debt_weight",
        "tax_rate",
    }
    _reject_unknown(arguments, required | {"precision"})
    _require_keys(arguments, required, "params.arguments")
    cost_of_equity = _decimal(
        arguments["cost_of_equity"], "params.arguments.cost_of_equity"
    )
    cost_of_debt = _decimal(arguments["cost_of_debt"], "params.arguments.cost_of_debt")
    equity_weight = _decimal(
        arguments["equity_weight"], "params.arguments.equity_weight"
    )
    debt_weight = _decimal(arguments["debt_weight"], "params.arguments.debt_weight")
    tax_rate = _decimal(arguments["tax_rate"], "params.arguments.tax_rate")
    if equity_weight < 0 or debt_weight < 0:
        raise invalid_params(
            "capital weights must be non-negative",
            path="params.arguments",
        )
    if abs((equity_weight + debt_weight) - Decimal(1)) > Decimal("0.0000001"):
        raise invalid_params(
            "equity_weight + debt_weight must equal 1",
            path="params.arguments",
        )
    if not Decimal(0) <= tax_rate < Decimal(1):
        raise invalid_params(
            "params.arguments.tax_rate must be in [0, 1)",
            path="params.arguments.tax_rate",
        )
    precision = _precision(arguments)
    control.progress("computing", 0, 1)
    with localcontext() as decimal_context:
        decimal_context.prec = 80
        value = (equity_weight * cost_of_equity) + (
            debt_weight * cost_of_debt * (Decimal(1) - tax_rate)
        )
    control.progress("computed", 1, 1)
    return ToolComputation(
        data={
            "kind": "rate",
            "value": _rounded_text(value, precision),
            "unit": "decimal",
            "formula": "WACC = E/V * cost_of_equity + D/V * cost_of_debt * (1 - tax_rate)",
            "inputs": {
                "cost_of_equity": str(cost_of_equity),
                "cost_of_debt": str(cost_of_debt),
                "equity_weight": str(equity_weight),
                "debt_weight": str(debt_weight),
                "tax_rate": str(tax_rate),
            },
        },
        used_source_ids=_all_source_ids(context),
    )


def _enterprise_value_bridge(
    arguments_value: object, context: DataContext, control: ExecutionControl
) -> ToolComputation:
    arguments = _require_arguments(arguments_value)
    allowed = {
        "enterprise_value",
        "equity_value",
        "net_debt",
        "minority_interest",
        "lease_liabilities",
        "non_operating_assets",
        "shares_outstanding",
        "currency",
        "precision",
    }
    _reject_unknown(arguments, allowed)
    _require_keys(arguments, {"net_debt", "currency"}, "params.arguments")
    has_ev = "enterprise_value" in arguments
    has_equity = "equity_value" in arguments
    if has_ev == has_equity:
        raise invalid_params(
            "provide exactly one of enterprise_value or equity_value",
            path="params.arguments",
        )
    net_debt = _decimal(arguments["net_debt"], "params.arguments.net_debt")
    minority = (
        _decimal(arguments["minority_interest"], "params.arguments.minority_interest")
        if "minority_interest" in arguments
        else Decimal(0)
    )
    leases = (
        _decimal(arguments["lease_liabilities"], "params.arguments.lease_liabilities")
        if "lease_liabilities" in arguments
        else Decimal(0)
    )
    non_operating = (
        _decimal(
            arguments["non_operating_assets"], "params.arguments.non_operating_assets"
        )
        if "non_operating_assets" in arguments
        else Decimal(0)
    )
    currency = _currency_code(arguments["currency"], "params.arguments.currency")
    shares = (
        _decimal(arguments["shares_outstanding"], "params.arguments.shares_outstanding")
        if "shares_outstanding" in arguments
        else None
    )
    if shares is not None and shares <= 0:
        raise invalid_params(
            "params.arguments.shares_outstanding must be greater than zero",
            path="params.arguments.shares_outstanding",
        )
    precision = _precision(arguments)
    control.progress("bridging", 0, 1)
    with localcontext() as decimal_context:
        decimal_context.prec = 80
        if has_ev:
            enterprise_value = _decimal(
                arguments["enterprise_value"], "params.arguments.enterprise_value"
            )
            equity_value = (
                enterprise_value - net_debt - minority - leases + non_operating
            )
        else:
            equity_value = _decimal(
                arguments["equity_value"], "params.arguments.equity_value"
            )
            enterprise_value = (
                equity_value + net_debt + minority + leases - non_operating
            )
        per_share = equity_value / shares if shares is not None else None
    control.progress("computed", 1, 1)
    data: dict[str, Any] = {
        "kind": "valuation",
        "currency": currency,
        "enterprise_value": _rounded_text(enterprise_value, precision),
        "equity_value": _rounded_text(equity_value, precision),
        "formula": "equity_value = enterprise_value - net_debt - minority_interest - lease_liabilities + non_operating_assets",
        "inputs": {
            "net_debt": str(net_debt),
            "minority_interest": str(minority),
            "lease_liabilities": str(leases),
            "non_operating_assets": str(non_operating),
            "shares_outstanding": str(shares) if shares is not None else None,
        },
    }
    if per_share is not None:
        data["per_share_value"] = _rounded_text(per_share, precision)
    return ToolComputation(
        data=data,
        used_source_ids=_all_source_ids(context),
    )


def _period_aggregate(
    arguments_value: object, context: DataContext, control: ExecutionControl
) -> ToolComputation:
    arguments = _require_arguments(arguments_value)
    _reject_unknown(arguments, {"values", "dates", "periods", "precision"})
    _require_keys(arguments, {"values", "dates"}, "params.arguments")
    values = _decimal_series(
        arguments["values"], "params.arguments.values", minimum=2, maximum=120
    )
    raw_dates = arguments["dates"]
    if not isinstance(raw_dates, list) or len(raw_dates) != len(values):
        raise invalid_params(
            "params.arguments.dates must align 1:1 with values",
            path="params.arguments.dates",
        )
    dates = [
        _parse_date(item, f"params.arguments.dates[{index}]")
        for index, item in enumerate(raw_dates)
    ]
    if dates != sorted(dates) or len(set(dates)) != len(dates):
        raise invalid_params(
            "params.arguments.dates must be unique and ascending",
            path="params.arguments.dates",
        )
    for index in range(1, len(dates)):
        if dates[index] == dates[index - 1]:
            raise invalid_params(
                "params.arguments.dates contains a duplicate period",
                path=f"params.arguments.dates[{index}]",
            )
    periods = arguments.get("periods", len(values))
    if (
        isinstance(periods, bool)
        or not isinstance(periods, int)
        or not 1 <= periods <= 40
    ):
        raise invalid_params(
            "params.arguments.periods must be an integer from 1 to 40",
            path="params.arguments.periods",
        )
    if periods > len(values):
        raise invalid_params(
            "params.arguments.periods exceeds the supplied series",
            path="params.arguments.periods",
        )
    precision = _precision(arguments)
    control.progress("aggregating", 0, 1)
    window = values[-periods:]
    total = sum(window, Decimal(0))
    control.progress("computed", 1, 1)
    return ToolComputation(
        data={
            "kind": "scalar",
            "value": _rounded_text(total, precision),
            "unit": "sum",
            "period_count": periods,
            "start": dates[-periods].isoformat(),
            "end": dates[-1].isoformat(),
            "formula": "sum(values[-periods:])",
            "inputs": {
                "periods": periods,
                "values": [str(item) for item in window],
            },
        },
        used_source_ids=_all_source_ids(context),
    )


def _currency_convert(
    arguments_value: object, context: DataContext, control: ExecutionControl
) -> ToolComputation:
    arguments = _require_arguments(arguments_value)
    required = {"amount", "currency", "quote_currency", "fx_rate", "fx_as_of"}
    _reject_unknown(arguments, required | {"precision"})
    _require_keys(arguments, required, "params.arguments")
    amount = _decimal(arguments["amount"], "params.arguments.amount")
    currency = _currency_code(arguments["currency"], "params.arguments.currency")
    quote = _currency_code(
        arguments["quote_currency"], "params.arguments.quote_currency"
    )
    fx_rate = _decimal(arguments["fx_rate"], "params.arguments.fx_rate")
    if fx_rate <= 0:
        raise invalid_params(
            "params.arguments.fx_rate must be greater than zero",
            path="params.arguments.fx_rate",
        )
    fx_as_of = _parse_date(arguments["fx_as_of"], "params.arguments.fx_as_of")
    if currency == quote:
        raise invalid_params(
            "currency and quote_currency must differ",
            path="params.arguments.quote_currency",
        )
    precision = _precision(arguments)
    control.progress("converting", 0, 1)
    converted = amount * fx_rate
    control.progress("computed", 1, 1)
    return ToolComputation(
        data={
            "kind": "scalar",
            "value": _rounded_text(converted, precision),
            "unit": quote,
            "formula": "amount * fx_rate",
            "inputs": {
                "amount": str(amount),
                "currency": currency,
                "quote_currency": quote,
                "fx_rate": str(fx_rate),
                "fx_as_of": fx_as_of.isoformat(),
            },
        },
        used_source_ids=_all_source_ids(context),
    )


def _dcf_core(
    arguments: dict[str, Any],
    *,
    discount_rate: Decimal,
    terminal_growth: Decimal | None,
    terminal_value_input: Decimal | None,
) -> tuple[Decimal, Decimal, Decimal, Decimal, list[Decimal], list[str]]:
    raw_cash_flows = arguments["cash_flows"]
    if not isinstance(raw_cash_flows, list) or not 1 <= len(raw_cash_flows) <= 30:
        raise invalid_params(
            "params.arguments.cash_flows must contain 1 to 30 annual values",
            path="params.arguments.cash_flows",
        )
    cash_flows = [
        _decimal(value, f"params.arguments.cash_flows[{index}]")
        for index, value in enumerate(raw_cash_flows)
    ]
    if discount_rate <= Decimal("-1"):
        raise invalid_params(
            "params.arguments.discount_rate must be greater than -1",
            path="params.arguments.discount_rate",
        )
    net_debt = _decimal(arguments.get("net_debt", "0"), "params.arguments.net_debt")
    with localcontext() as decimal_context:
        decimal_context.prec = 80
        base = Decimal(1) + discount_rate
        present_values = [
            cash_flow / (base**index)
            for index, cash_flow in enumerate(cash_flows, start=1)
        ]
        if terminal_growth is not None:
            if terminal_growth >= discount_rate or cash_flows[-1] <= 0:
                raise invalid_params(
                    "Gordon growth requires terminal_growth_rate < discount_rate and a positive final cash flow",
                    path="params.arguments.terminal_growth_rate",
                )
            terminal_value = (
                cash_flows[-1]
                * (Decimal(1) + terminal_growth)
                / (discount_rate - terminal_growth)
            )
        elif terminal_value_input is not None:
            terminal_value = terminal_value_input
        else:
            terminal_value = Decimal(0)
        terminal_present_value = terminal_value / (base ** len(cash_flows))
        enterprise_value = sum(present_values, Decimal(0)) + terminal_present_value
        equity_value = enterprise_value - net_debt
    warnings: list[str] = []
    if enterprise_value > 0 and terminal_present_value > 0:
        if terminal_present_value / enterprise_value > Decimal("0.8"):
            warnings.append("terminal_value_exceeds_80_percent_of_enterprise_value")
    return (
        enterprise_value,
        equity_value,
        terminal_value,
        terminal_present_value,
        present_values,
        warnings,
    )


def _dcf_sensitivity(
    arguments_value: object, context: DataContext, control: ExecutionControl
) -> ToolComputation:
    arguments = _require_arguments(arguments_value)
    allowed = {
        "cash_flows",
        "discount_rate",
        "terminal_growth_rate",
        "terminal_value",
        "net_debt",
        "shares_outstanding",
        "currency",
        "discount_rate_shocks",
        "growth_rate_shocks",
        "precision",
    }
    _reject_unknown(arguments, allowed)
    _require_keys(
        arguments,
        {
            "cash_flows",
            "discount_rate",
            "currency",
            "discount_rate_shocks",
            "growth_rate_shocks",
        },
        "params.arguments",
    )
    if "terminal_growth_rate" in arguments and "terminal_value" in arguments:
        raise invalid_params(
            "provide terminal_growth_rate or terminal_value, not both",
            path="params.arguments",
        )
    base_rate = _decimal(arguments["discount_rate"], "params.arguments.discount_rate")
    base_growth = (
        _decimal(
            arguments["terminal_growth_rate"], "params.arguments.terminal_growth_rate"
        )
        if "terminal_growth_rate" in arguments
        else None
    )
    terminal_value_input = (
        _decimal(arguments["terminal_value"], "params.arguments.terminal_value")
        if "terminal_value" in arguments
        else None
    )
    currency = _currency_code(arguments["currency"], "params.arguments.currency")
    rate_shocks = _decimal_series(
        arguments["discount_rate_shocks"],
        "params.arguments.discount_rate_shocks",
        minimum=1,
        maximum=8,
    )
    growth_shocks = _decimal_series(
        arguments["growth_rate_shocks"],
        "params.arguments.growth_rate_shocks",
        minimum=1,
        maximum=8,
    )
    precision = _precision(arguments)
    grid: list[dict[str, Any]] = []
    warnings: list[str] = []
    total = len(rate_shocks) * len(growth_shocks)
    done = 0
    for rate_shock in rate_shocks:
        for growth_shock in growth_shocks:
            rate = base_rate + rate_shock
            growth = None if base_growth is None else base_growth + growth_shock
            enterprise, equity, _terminal, _tpv, _pvs, cell_warnings = _dcf_core(
                arguments,
                discount_rate=rate,
                terminal_growth=growth,
                terminal_value_input=terminal_value_input,
            )
            warnings.extend(cell_warnings)
            grid.append(
                {
                    "discount_rate": _rounded_text(rate, precision),
                    "terminal_growth_rate": None
                    if growth is None
                    else _rounded_text(growth, precision),
                    "enterprise_value": _rounded_text(enterprise, precision),
                    "equity_value": _rounded_text(equity, precision),
                }
            )
            done += 1
            control.progress("grid", done, total)
    return ToolComputation(
        data={
            "kind": "sensitivity_grid",
            "currency": currency,
            "cells": grid,
            "formula": "DCF enterprise and equity values on the supplied shock grid",
        },
        used_source_ids=_all_source_ids(context),
        warnings=tuple(dict.fromkeys(warnings)),
    )


def _risk_metrics(
    arguments_value: object, context: DataContext, control: ExecutionControl
) -> ToolComputation:
    arguments = _require_arguments(arguments_value)
    _reject_unknown(arguments, {"values", "market_values", "dates", "precision"})
    _require_keys(arguments, {"values", "market_values"}, "params.arguments")
    values = _decimal_series(
        arguments["values"], "params.arguments.values", minimum=3, maximum=6000
    )
    market = _decimal_series(
        arguments["market_values"],
        "params.arguments.market_values",
        minimum=3,
        maximum=6000,
    )
    if len(values) != len(market):
        raise invalid_params(
            "values and market_values must have the same length",
            path="params.arguments.market_values",
        )
    if any(item <= 0 for item in values + market):
        raise invalid_params(
            "price series must be strictly positive",
            path="params.arguments.values",
        )
    precision = _precision(arguments)
    control.progress("computing", 0, 1)
    asset_returns = _returns(values, "params.arguments.values")
    market_returns = _returns(market, "params.arguments.market_values")
    with localcontext() as decimal_context:
        decimal_context.prec = 80
        asset_mean = sum(asset_returns, Decimal(0)) / Decimal(len(asset_returns))
        market_mean = sum(market_returns, Decimal(0)) / Decimal(len(market_returns))
        covariance = sum(
            (left - asset_mean) * (right - market_mean)
            for left, right in zip(asset_returns, market_returns, strict=True)
        ) / Decimal(len(asset_returns) - 1)
        asset_vol = _population_stdev(asset_returns)
        market_vol = _population_stdev(market_returns)
        if market_vol == 0:
            raise invalid_params(
                "market series has zero variance",
                path="params.arguments.market_values",
            )
        correlation = covariance / (asset_vol * market_vol)
        market_var = market_vol**2
        beta = covariance / market_var
    control.progress("computed", 1, 1)
    return ToolComputation(
        data={
            "kind": "risk_metrics",
            "correlation": _rounded_text(correlation, precision),
            "beta": _rounded_text(beta, precision),
            "observation_count": len(values),
            "return_count": len(asset_returns),
            "formula": "beta = cov(r_a, r_m) / var(r_m); correlation = cov / (s_a * s_m)",
        },
        used_source_ids=_all_source_ids(context),
    )


CALCULATION_SPECS = (
    PERCENTAGE_CHANGE_SPEC,
    RATIO_SPEC,
    CAGR_SPEC,
    DISCOUNTED_CASH_FLOW_SPEC,
    POINT_IN_TIME_FILTER_SPEC,
    SERIES_STATISTICS_SPEC,
    INTERNAL_RATE_OF_RETURN_SPEC,
    WACC_SPEC,
    ENTERPRISE_VALUE_BRIDGE_SPEC,
    PERIOD_AGGREGATE_SPEC,
    CURRENCY_CONVERT_SPEC,
    DCF_SENSITIVITY_SPEC,
    RISK_METRICS_SPEC,
)

CALCULATION_FUNCTIONS = {
    "percentage_change": _percentage_change,
    "ratio": _ratio,
    "compound_annual_growth_rate": _compound_annual_growth_rate,
    "discounted_cash_flow": _discounted_cash_flow,
    "point_in_time_filter": _point_in_time_filter,
    "series_statistics": _series_statistics,
    "internal_rate_of_return": _internal_rate_of_return,
    "weighted_average_cost_of_capital": _weighted_average_cost_of_capital,
    "enterprise_value_bridge": _enterprise_value_bridge,
    "period_aggregate": _period_aggregate,
    "currency_convert": _currency_convert,
    "dcf_sensitivity": _dcf_sensitivity,
    "risk_metrics": _risk_metrics,
}
