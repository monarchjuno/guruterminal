#!/usr/bin/env python3
"""Build Guru Terminal's pinned, read-only Korea Investment API catalog.

The upstream Trading MCP downloads Python at runtime. Guru Terminal does not. This
generator reviews the pinned upstream configs and Python examples at build time,
rejects every POST operation, and emits the exact GET request contract consumed by
the native connector.
"""

from __future__ import annotations

import argparse
import ast
from collections import Counter
import json
from pathlib import Path
import subprocess
import sys
from typing import Any


UPSTREAM_REPOSITORY = "https://github.com/koreainvestment/open-trading-api"
UPSTREAM_COMMIT = "b093e42ba32d1df5f5ddad7a71cb715cbc800832"
SCHEMA = "guruterminal-kis-read-api/1"
EXPECTED_TOTAL = 164
EXPECTED_WRITES = 18
EXPECTED_READS = 146
EXPECTED_MARKET_READS = 91
EXPECTED_ACCOUNT_READS = 55

PRODUCTS = (
    "domestic_bond",
    "domestic_futureoption",
    "domestic_stock",
    "elw",
    "etfetn",
    "overseas_futureoption",
    "overseas_stock",
)

EXPECTED_WRITE_OPERATION_IDS = {
    "domestic_bond.buy",
    "domestic_bond.order_rvsecncl",
    "domestic_bond.sell",
    "domestic_futureoption.order",
    "domestic_futureoption.order_rvsecncl",
    "domestic_stock.order_cash",
    "domestic_stock.order_credit",
    "domestic_stock.order_resv",
    "domestic_stock.order_resv_rvsecncl",
    "domestic_stock.order_rvsecncl",
    "overseas_futureoption.order",
    "overseas_futureoption.order_rvsecncl",
    "overseas_stock.daytime_order",
    "overseas_stock.daytime_order_rvsecncl",
    "overseas_stock.order",
    "overseas_stock.order_resv",
    "overseas_stock.order_resv_ccnl",
    "overseas_stock.order_rvsecncl",
}

# Values for these parameters come from a native, encrypted account profile. They
# are never accepted from a model tool call, even for the three quote endpoints
# whose upstream examples use an HTS user ID.
ACCOUNT_PROFILE_PARAMETERS = {
    "acnt_prdt_cd": "account_product_code",
    "acnt_pwd": "account_password",
    "cano": "account_number",
    "cust_rncno25": "customer_identity_number",
    "hmid": "home_net_id",
    "user_id": "hts_id",
}

INTERNAL_PARAMETERS = {
    "depth",
    "env_dv",
    "max_depth",
    "tr_cont",
}


class ContractError(RuntimeError):
    pass


def fail(message: str) -> None:
    raise ContractError(message)


def literal(node: ast.AST, context: str) -> Any:
    try:
        return ast.literal_eval(node)
    except (ValueError, TypeError) as error:
        fail(f"{context}: expected a literal, got {ast.unparse(node)!r}: {error}")


def is_internal_parameter(name: str) -> bool:
    return name in INTERNAL_PARAMETERS or name.startswith("dataframe")


def normalize_type(value: str | None) -> str:
    value = value or "str"
    if "int" in value:
        return "integer"
    if "float" in value:
        return "number"
    if "bool" in value:
        return "boolean"
    return "string"


def git_head(source: Path) -> str:
    result = subprocess.run(
        ["git", "-C", str(source), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def require_clean_checkout(source: Path) -> None:
    result = subprocess.run(
        ["git", "-C", str(source), "status", "--porcelain", "--untracked-files=all"],
        check=True,
        capture_output=True,
        text=True,
    )
    if result.stdout:
        fail("source checkout must be clean; refusing locally modified upstream input")


def find_function(tree: ast.Module, name: str, operation_id: str) -> ast.FunctionDef:
    matches = [
        node
        for node in tree.body
        if isinstance(node, ast.FunctionDef) and node.name == name
    ]
    if len(matches) != 1:
        fail(
            f"{operation_id}: expected one function named {name}, found {len(matches)}"
        )
    return matches[0]


def find_api_url(tree: ast.Module, operation_id: str) -> str:
    values: list[str] = []
    for node in tree.body:
        if not isinstance(node, ast.Assign):
            continue
        if any(
            isinstance(target, ast.Name) and target.id == "API_URL"
            for target in node.targets
        ):
            value = literal(node.value, f"{operation_id} API_URL")
            if not isinstance(value, str):
                fail(f"{operation_id}: API_URL must be a string")
            values.append(value)
    if len(values) != 1:
        fail(f"{operation_id}: expected one API_URL, found {len(values)}")
    return values[0]


def find_url_fetch(function: ast.FunctionDef, operation_id: str) -> ast.Call:
    calls = [
        node
        for node in ast.walk(function)
        if isinstance(node, ast.Call) and ast.unparse(node.func) == "ka._url_fetch"
    ]
    if len(calls) != 1:
        fail(f"{operation_id}: expected one ka._url_fetch call, found {len(calls)}")
    return calls[0]


def find_module_assignment(tree: ast.Module, name: str, operation_id: str) -> ast.AST:
    values: list[ast.AST] = []
    for node in tree.body:
        if isinstance(node, ast.Assign) and any(
            isinstance(target, ast.Name) and target.id == name
            for target in node.targets
        ):
            values.append(node.value)
        elif (
            isinstance(node, ast.AnnAssign)
            and isinstance(node.target, ast.Name)
            and node.target.id == name
            and node.value is not None
        ):
            values.append(node.value)
    if len(values) != 1:
        fail(f"{operation_id}: expected one {name} assignment, found {len(values)}")
    return values[0]


def extract_response_contract(
    source: Path,
    product: str,
    api_key: str,
    method: str,
    function: ast.FunctionDef,
    operation_id: str,
) -> dict[str, Any]:
    containers: set[str] = set()
    top_level_fields: set[str] = set()
    for node in ast.walk(function):
        if not (
            isinstance(node, ast.Attribute)
            and isinstance(node.value, ast.Call)
            and not node.value.args
            and not node.value.keywords
            and isinstance(node.value.func, ast.Attribute)
            and node.value.func.attr == "getBody"
            and isinstance(node.value.func.value, ast.Name)
            and node.value.func.value.id == "res"
        ):
            continue
        if node.attr == "output" or (
            node.attr.startswith("output")
            and node.attr.removeprefix("output").isdigit()
        ):
            containers.add(node.attr)
        else:
            top_level_fields.add(node.attr)
    if not containers:
        fail(f"{operation_id}: implementation exposes no static output container")

    relative_check_file = Path("examples_llm") / product / api_key / f"chk_{method}.py"
    check_file = source / relative_check_file
    if not check_file.is_file():
        fail(f"{operation_id}: missing official response check {check_file}")
    check_tree = ast.parse(
        check_file.read_text(encoding="utf-8"), filename=str(check_file)
    )
    imports_method = any(
        isinstance(node, ast.ImportFrom)
        and node.module == method
        and any(alias.name == method for alias in node.names)
        for node in check_tree.body
    )
    if not imports_method:
        fail(f"{operation_id}: response check does not import its implementation")

    mapping_node = find_module_assignment(check_tree, "COLUMN_MAPPING", operation_id)
    if not isinstance(mapping_node, ast.Dict):
        fail(f"{operation_id}: COLUMN_MAPPING must be a dict literal")
    allowed_fields: list[str] = []
    for key_node, value_node in zip(
        mapping_node.keys, mapping_node.values, strict=True
    ):
        field = literal(key_node, f"{operation_id} COLUMN_MAPPING key")
        label = literal(value_node, f"{operation_id} COLUMN_MAPPING value")
        if (
            not isinstance(field, str)
            or not field
            or len(field) > 128
            or not field.replace("_", "").isalnum()
            or not field.isascii()
            or not isinstance(label, str)
        ):
            fail(f"{operation_id}: COLUMN_MAPPING has an invalid field declaration")
        allowed_fields.append(field)
    if not allowed_fields:
        fail(f"{operation_id}: COLUMN_MAPPING must not be empty")

    numeric_node = find_module_assignment(check_tree, "NUMERIC_COLUMNS", operation_id)
    numeric_columns = literal(numeric_node, f"{operation_id} NUMERIC_COLUMNS")
    if not isinstance(numeric_columns, list) or any(
        not isinstance(column, str) for column in numeric_columns
    ):
        fail(f"{operation_id}: NUMERIC_COLUMNS must be a static string list")

    # The official checks use one mapping for every DataFrame returned by an
    # operation. They do not declare container-specific field membership, so the
    # only faithful allowlist is the operation-wide union below.
    return {
        "containers": sorted(containers),
        "top_level_fields": sorted(top_level_fields),
        "allowed_fields": sorted(set(allowed_fields)),
        "field_scope": "operation_union",
        "source": f"{relative_check_file.as_posix()}:COLUMN_MAPPING",
        "complete": True,
        "unknown_fields": "drop",
        "identity_checks": [],
    }


def is_post(call: ast.Call, operation_id: str) -> bool:
    post_keywords = [keyword for keyword in call.keywords if keyword.arg == "postFlag"]
    if not post_keywords:
        return False
    if len(post_keywords) != 1:
        fail(f"{operation_id}: multiple postFlag arguments")
    value = literal(post_keywords[0].value, f"{operation_id} postFlag")
    if not isinstance(value, bool):
        fail(f"{operation_id}: postFlag must be a boolean literal")
    return value


def parse_equality(test: ast.AST, operation_id: str) -> tuple[str, Any]:
    if isinstance(test, ast.BoolOp) and isinstance(test.op, ast.Or):
        comparisons = [parse_equality(value, operation_id) for value in test.values]
        names = {name for name, _ in comparisons}
        values = {value for _, value in comparisons}
        if names == {"env_dv"} and values == {"real", "demo"}:
            return "env_dv", "real_or_demo"
    if (
        isinstance(test, ast.Compare)
        and len(test.ops) == 1
        and isinstance(test.ops[0], ast.Eq)
        and len(test.comparators) == 1
        and isinstance(test.left, ast.Name)
    ):
        value = literal(test.comparators[0], f"{operation_id} TR-ID condition")
        if not isinstance(value, (str, int, bool)):
            fail(f"{operation_id}: unsupported TR-ID condition value {value!r}")
        return test.left.id, value
    fail(f"{operation_id}: unsupported dynamic TR-ID condition {ast.unparse(test)!r}")


def extract_tr_id_rules(
    function: ast.FunctionDef, operation_id: str
) -> tuple[list[dict[str, Any]], set[str]]:
    rows: list[tuple[str, list[tuple[str, str, Any]]]] = []

    class Visitor(ast.NodeVisitor):
        def __init__(self) -> None:
            self.conditions: list[tuple[str, str, Any]] = []

        def visit_If(self, node: ast.If) -> None:  # noqa: N802
            # Input validation and response/pagination branches cannot choose a
            # request TR ID. Avoid trying to interpret their arbitrary boolean
            # expressions as routing rules.
            if not any(
                isinstance(child, ast.Assign)
                and any(
                    isinstance(target, ast.Name) and target.id == "tr_id"
                    for target in child.targets
                )
                for child in ast.walk(node)
            ):
                self.generic_visit(node)
                return
            name, value = parse_equality(node.test, operation_id)
            self.conditions.append((name, "eq", value))
            for child in node.body:
                self.visit(child)
            self.conditions.pop()

            self.conditions.append((name, "neq", value))
            for child in node.orelse:
                self.visit(child)
            self.conditions.pop()

        def visit_Assign(self, node: ast.Assign) -> None:  # noqa: N802
            if any(
                isinstance(target, ast.Name) and target.id == "tr_id"
                for target in node.targets
            ):
                value = literal(node.value, f"{operation_id} tr_id")
                if not isinstance(value, str) or not value:
                    fail(f"{operation_id}: tr_id must be a non-empty string literal")
                rows.append((value, list(self.conditions)))
            self.generic_visit(node)

    Visitor().visit(function)
    if not rows:
        fail(f"{operation_id}: no static tr_id assignment found")

    raw_rules: list[dict[str, Any]] = []
    routing_parameters: set[str] = set()
    for value, conditions in rows:
        positives: dict[str, Any] = {}
        negatives: list[tuple[str, Any]] = []
        for name, operator, expected in conditions:
            if operator == "eq":
                if name == "env_dv" and expected == "real_or_demo":
                    continue
                previous = positives.get(name)
                if previous is not None and previous != expected:
                    fail(f"{operation_id}: contradictory TR-ID rule for {name}")
                positives[name] = expected
            else:
                negatives.append((name, expected))
        for name, expected in negatives:
            if name not in positives:
                if name == "env_dv" and expected == "real_or_demo":
                    continue
                fail(
                    f"{operation_id}: TR-ID assignment depends only on "
                    f"{name} != {expected!r}, which cannot be represented exactly"
                )
        when: dict[str, Any] = {}
        for name, expected in positives.items():
            exposed_name = "environment" if name == "env_dv" else name
            when[exposed_name] = expected
            if name != "env_dv":
                routing_parameters.add(name)
        raw_rules.append({"when": when, "value": value})

    # The KIS host is selected independently from many upstream helper functions.
    # Make both live and demo resolution explicit, even when their TR ID is equal.
    expanded: list[dict[str, Any]] = []
    for rule in raw_rules:
        if "environment" in rule["when"]:
            expanded.append(rule)
            continue
        for environment in ("real", "demo"):
            when = {"environment": environment, **rule["when"]}
            expanded.append({"when": when, "value": rule["value"]})

    seen: set[str] = set()
    rules: list[dict[str, Any]] = []
    for rule in expanded:
        key = json.dumps(rule, ensure_ascii=False, sort_keys=True)
        if key not in seen:
            seen.add(key)
            rules.append(rule)
    environments = {rule["when"].get("environment") for rule in rules}
    if environments != {"real", "demo"}:
        fail(f"{operation_id}: TR-ID rules do not cover real and demo environments")
    return rules, routing_parameters


def parse_doc_parameters(function: ast.FunctionDef) -> dict[str, str]:
    doc = ast.get_docstring(function) or ""
    descriptions: dict[str, str] = {}
    in_args = False
    current: str | None = None
    for raw_line in doc.splitlines():
        line = raw_line.strip()
        if line == "Args:":
            in_args = True
            current = None
            continue
        if in_args and line.endswith(":") and not line.startswith(("*", "**")):
            break
        if not in_args:
            continue
        if ":" in line and "(" in line.split(":", 1)[0]:
            head, description = line.split(":", 1)
            name = head.split("(", 1)[0].strip().lstrip("*")
            if name:
                current = name
                descriptions[name] = description.strip()
                continue
        if current and line:
            descriptions[current] = f"{descriptions[current]} {line}".strip()
    return descriptions


def signature_parameters(
    function: ast.FunctionDef, operation_id: str
) -> tuple[list[str], dict[str, dict[str, Any]]]:
    positional = list(function.args.posonlyargs) + list(function.args.args)
    keyword_only = list(function.args.kwonlyargs)
    names = [argument.arg for argument in positional + keyword_only]
    defaults: dict[str, ast.AST | None] = {name: None for name in names}

    for argument, default in zip(
        positional[-len(function.args.defaults) :] if function.args.defaults else [],
        function.args.defaults,
        strict=True,
    ):
        defaults[argument.arg] = default
    for argument, default in zip(keyword_only, function.args.kw_defaults, strict=True):
        defaults[argument.arg] = default

    result: dict[str, dict[str, Any]] = {}
    for argument in positional + keyword_only:
        default_node = defaults[argument.arg]
        default_value: Any = None
        if default_node is not None:
            default_value = literal(
                default_node, f"{operation_id} default for {argument.arg}"
            )
        result[argument.arg] = {
            "required": default_node is None,
            "default": default_value,
            "annotation": ast.unparse(argument.annotation)
            if argument.annotation
            else "str",
        }
    return names, result


def extract_query_mappings(
    function: ast.FunctionDef, operation_id: str
) -> tuple[list[dict[str, Any]], list[str]]:
    mappings: list[tuple[int, int, dict[str, Any]]] = []
    dynamic_parameter_names: list[str] = []
    sequence = 0

    class Visitor(ast.NodeVisitor):
        def __init__(self) -> None:
            self.send_modes: list[str] = []

        def add(self, node: ast.AST, value: dict[str, Any]) -> None:
            nonlocal sequence
            mappings.append((getattr(node, "lineno", 0), sequence, value))
            sequence += 1

        def visit_If(self, node: ast.If) -> None:  # noqa: N802
            if isinstance(node.test, ast.Name):
                send_mode = "if_nonempty"
            elif (
                isinstance(node.test, ast.Compare)
                and len(node.test.ops) == 1
                and isinstance(node.test.ops[0], ast.IsNot)
                and len(node.test.comparators) == 1
                and isinstance(node.test.left, ast.Name)
                and isinstance(node.test.comparators[0], ast.Constant)
                and node.test.comparators[0].value is None
            ):
                send_mode = "if_present"
            else:
                # Most if statements are validation and response handling. Only
                # assignments to params consume this marker below.
                send_mode = "conditional"
            self.send_modes.append(send_mode)
            for child in node.body:
                self.visit(child)
            self.send_modes.pop()
            for child in node.orelse:
                self.visit(child)

        def visit_Assign(self, node: ast.Assign) -> None:  # noqa: N802
            params_targets = [
                target
                for target in node.targets
                if isinstance(target, ast.Name) and target.id == "params"
            ]
            if params_targets:
                if not isinstance(node.value, ast.Dict):
                    fail(
                        f"{operation_id}: params must be initialized with a dict literal"
                    )
                for key_node, value_node in zip(
                    node.value.keys, node.value.values, strict=True
                ):
                    wire_name = literal(key_node, f"{operation_id} query key")
                    if not isinstance(wire_name, str) or not wire_name:
                        fail(f"{operation_id}: query key must be a non-empty string")
                    if isinstance(value_node, ast.Name):
                        self.add(
                            key_node,
                            {
                                "wire_name": wire_name,
                                "parameter": value_node.id,
                                "send": "always",
                            },
                        )
                    else:
                        self.add(
                            key_node,
                            {
                                "wire_name": wire_name,
                                "literal": literal(
                                    value_node,
                                    f"{operation_id} query literal {wire_name}",
                                ),
                                "send": "always",
                            },
                        )

            for target in node.targets:
                if not (
                    isinstance(target, ast.Subscript)
                    and isinstance(target.value, ast.Name)
                    and target.value.id == "params"
                ):
                    continue
                # Non-literal subscripts are expanded by the restricted loop parser.
                if not isinstance(target.slice, ast.Constant):
                    continue
                wire_name = literal(target.slice, f"{operation_id} dynamic query key")
                if not isinstance(wire_name, str) or not wire_name:
                    fail(f"{operation_id}: dynamic query key must be a string")
                if not isinstance(node.value, ast.Name):
                    fail(
                        f"{operation_id}: unsupported dynamic query value "
                        f"{ast.unparse(node.value)!r}"
                    )
                send_mode = self.send_modes[-1] if self.send_modes else "always"
                if send_mode == "conditional":
                    fail(
                        f"{operation_id}: unsupported conditional query mapping "
                        f"{ast.unparse(node)!r}"
                    )
                self.add(
                    target,
                    {
                        "wire_name": wire_name,
                        "parameter": node.value.id,
                        "send": send_mode,
                    },
                )
            self.generic_visit(node)

    Visitor().visit(function)

    # One upstream helper deliberately accepts srs_cd_01..32 through **kwargs and
    # constructs both names in a fixed range. Expand that bounded pattern here.
    for node in ast.walk(function):
        if not isinstance(node, ast.For):
            continue
        if not (
            isinstance(node.target, ast.Name)
            and isinstance(node.iter, ast.Call)
            and isinstance(node.iter.func, ast.Name)
            and node.iter.func.id == "range"
        ):
            continue
        range_args = [literal(arg, f"{operation_id} range") for arg in node.iter.args]
        values = list(range(*range_args))
        local_templates: dict[str, ast.JoinedStr] = {}
        kwargs_assignment: ast.Assign | None = None
        for child in node.body:
            if (
                isinstance(child, ast.Assign)
                and len(child.targets) == 1
                and isinstance(child.targets[0], ast.Name)
                and isinstance(child.value, ast.JoinedStr)
            ):
                local_templates[child.targets[0].id] = child.value
            if isinstance(child, ast.Assign):
                for target in child.targets:
                    if (
                        isinstance(target, ast.Subscript)
                        and isinstance(target.value, ast.Name)
                        and target.value.id == "params"
                        and not isinstance(target.slice, ast.Constant)
                    ):
                        kwargs_assignment = child
        if kwargs_assignment is None:
            continue
        target = kwargs_assignment.targets[0]
        assert isinstance(target, ast.Subscript)
        if not (
            isinstance(target.slice, ast.Name)
            and target.slice.id in local_templates
            and isinstance(kwargs_assignment.value, ast.Call)
            and isinstance(kwargs_assignment.value.func, ast.Attribute)
            and isinstance(kwargs_assignment.value.func.value, ast.Name)
            and kwargs_assignment.value.func.value.id == "kwargs"
            and kwargs_assignment.value.func.attr == "get"
            and kwargs_assignment.value.args
            and isinstance(kwargs_assignment.value.args[0], ast.Name)
            and kwargs_assignment.value.args[0].id in local_templates
        ):
            fail(f"{operation_id}: unsupported dynamic params loop")

        def render(template: ast.JoinedStr, loop_value: int) -> str:
            pieces: list[str] = []
            for value in template.values:
                if isinstance(value, ast.Constant) and isinstance(value.value, str):
                    pieces.append(value.value)
                elif (
                    isinstance(value, ast.FormattedValue)
                    and isinstance(value.value, ast.Name)
                    and value.value.id == node.target.id
                ):
                    format_spec = ""
                    if value.format_spec is not None:
                        if not all(
                            isinstance(piece, ast.Constant)
                            and isinstance(piece.value, str)
                            for piece in value.format_spec.values
                        ):
                            fail(f"{operation_id}: dynamic format spec must be static")
                        format_spec = "".join(
                            piece.value for piece in value.format_spec.values
                        )
                    pieces.append(format(loop_value, format_spec))
                else:
                    fail(f"{operation_id}: unsupported dynamic f-string")
            return "".join(pieces)

        parameter_template = local_templates[kwargs_assignment.value.args[0].id]
        wire_template = local_templates[target.slice.id]
        for offset, loop_value in enumerate(values):
            parameter_name = render(parameter_template, loop_value)
            wire_name = render(wire_template, loop_value)
            dynamic_parameter_names.append(parameter_name)
            mappings.append(
                (
                    node.lineno,
                    sequence + offset,
                    {
                        "wire_name": wire_name,
                        "parameter": parameter_name,
                        "send": "always",
                    },
                )
            )
        sequence += len(values)

    mappings.sort(key=lambda row: (row[0], row[1]))
    values = [row[2] for row in mappings]
    wire_names = [mapping["wire_name"] for mapping in values]
    if len(wire_names) != len(set(wire_names)):
        duplicates = [name for name, count in Counter(wire_names).items() if count > 1]
        fail(f"{operation_id}: duplicate query mappings: {duplicates}")
    return values, dynamic_parameter_names


def parameter_source(name: str, query_wire_names: list[str]) -> tuple[str, str | None]:
    if name in ACCOUNT_PROFILE_PARAMETERS:
        return "account_profile", ACCOUNT_PROFILE_PARAMETERS[name]
    if any(wire.startswith("CTX_AREA_") or wire == "CTS" for wire in query_wire_names):
        return "continuation", None
    return "tool", None


def build_parameters(
    function: ast.FunctionDef,
    api: dict[str, Any],
    query: list[dict[str, Any]],
    routing_parameters: set[str],
    dynamic_parameter_names: list[str],
    operation_id: str,
) -> list[dict[str, Any]]:
    signature_order, signature = signature_parameters(function, operation_id)
    config = api["params"]
    docs = parse_doc_parameters(function)
    mapped_names = {mapping["parameter"] for mapping in query if "parameter" in mapping}
    needed = mapped_names | routing_parameters

    parameter_order = [
        name
        for name in signature_order
        if not is_internal_parameter(name) and name in needed
    ]
    for name in dynamic_parameter_names:
        if name not in parameter_order:
            parameter_order.append(name)

    missing = needed - set(parameter_order)
    if missing:
        fail(
            f"{operation_id}: query or routing parameters absent from signature: {sorted(missing)}"
        )

    ignored_config = {
        name for name in config if is_internal_parameter(name) or name == "env_dv"
    }
    unused_config = set(config) - set(parameter_order) - ignored_config
    if unused_config:
        fail(
            f"{operation_id}: upstream config parameters are not represented: {sorted(unused_config)}"
        )

    result: list[dict[str, Any]] = []
    for name in parameter_order:
        metadata = config.get(name, {})
        signature_metadata = signature.get(name)
        if signature_metadata is None:
            # The only such parameters are the bounded **kwargs expansion above.
            signature_metadata = {
                "required": False,
                "default": "",
                "annotation": "str",
            }
        query_wire_names = [
            mapping["wire_name"]
            for mapping in query
            if mapping.get("parameter") == name
        ]
        source, profile_key = parameter_source(name, query_wire_names)
        description = metadata.get("description") or docs.get(name)
        if not description and name.startswith("srs_cd_"):
            description = f"품목종류 코드 {name.removeprefix('srs_cd_')}"
        if not description:
            description = f"Upstream parameter {name}."
        item: dict[str, Any] = {
            "id": name,
            "type": normalize_type(
                metadata.get("type") or signature_metadata["annotation"]
            ),
            "required": bool(signature_metadata["required"]),
            "default": signature_metadata["default"],
            "description": description,
            "source": source,
        }
        if profile_key is not None:
            item["profile_key"] = profile_key
        result.append(item)
    return result


def build_operation(
    source: Path,
    product: str,
    api_key: str,
    api: dict[str, Any],
) -> tuple[dict[str, Any] | None, bool]:
    operation_id = f"{product}.{api_key}"
    source_file = source / "examples_llm" / product / api_key / f"{api['method']}.py"
    if not source_file.is_file():
        fail(f"{operation_id}: missing source file {source_file}")
    tree = ast.parse(source_file.read_text(encoding="utf-8"), filename=str(source_file))
    function = find_function(tree, api["method"], operation_id)
    api_url = find_api_url(tree, operation_id)
    if api_url != api["api_path"]:
        fail(
            f"{operation_id}: config path {api['api_path']!r} does not match "
            f"source API_URL {api_url!r}"
        )
    if not api_url.startswith("/uapi/") or "://" in api_url:
        fail(f"{operation_id}: API path is not a fixed relative /uapi/ path")
    call = find_url_fetch(function, operation_id)
    if is_post(call, operation_id):
        return None, True

    tr_id_rules, routing_parameters = extract_tr_id_rules(function, operation_id)
    query, dynamic_parameter_names = extract_query_mappings(function, operation_id)
    parameters = build_parameters(
        function,
        api,
        query,
        routing_parameters,
        dynamic_parameter_names,
        operation_id,
    )
    response = extract_response_contract(
        source,
        product,
        api_key,
        api["method"],
        function,
        operation_id,
    )
    scope = "account" if "주문/계좌" in api["category"] else "market"
    profile_parameter_ids = {
        parameter["id"]
        for parameter in parameters
        if parameter["source"] == "account_profile"
    }
    if scope == "account" and not {"cano", "acnt_prdt_cd"}.issubset(
        profile_parameter_ids
    ):
        fail(f"{operation_id}: account operation lacks native account/profile fields")

    continuation_wire_names = [
        mapping["wire_name"]
        for mapping in query
        if "parameter" in mapping
        and next(
            parameter
            for parameter in parameters
            if parameter["id"] == mapping["parameter"]
        )["source"]
        == "continuation"
    ]
    operation: dict[str, Any] = {
        "id": operation_id,
        "product": product,
        "category": api["category"],
        "name": api["name"],
        "scope": scope,
        "http_method": "GET",
        "path": api_url,
        "tr_id_rules": tr_id_rules,
        "parameters": parameters,
        "query": query,
        "response": response,
    }
    if continuation_wire_names or "tr_cont" in {
        argument.arg for argument in function.args.args + function.args.kwonlyargs
    }:
        operation["continuation"] = {
            "request_header": "tr_cont",
            "response_header": "tr_cont",
            "query_fields": continuation_wire_names,
        }
    return operation, False


def build_manifest(source: Path) -> dict[str, Any]:
    head = git_head(source)
    if head != UPSTREAM_COMMIT:
        fail(f"source checkout must be exactly {UPSTREAM_COMMIT}; found {head}")
    require_clean_checkout(source)
    config_root = source / "MCP" / "Kis Trading MCP" / "configs"
    operations: list[dict[str, Any]] = []
    writes: set[str] = set()
    total = 0
    for product in PRODUCTS:
        config_path = config_root / f"{product}.json"
        payload = json.loads(config_path.read_text(encoding="utf-8"))
        for api_key, api in payload["apis"].items():
            total += 1
            operation, write = build_operation(source, product, api_key, api)
            operation_id = f"{product}.{api_key}"
            if write:
                writes.add(operation_id)
            else:
                assert operation is not None
                operations.append(operation)

    counts = Counter(operation["scope"] for operation in operations)
    operation_ids = [operation["id"] for operation in operations]
    if total != EXPECTED_TOTAL:
        fail(f"expected {EXPECTED_TOTAL} upstream operations, found {total}")
    if writes != EXPECTED_WRITE_OPERATION_IDS:
        fail(
            "write allow-deny set changed; review upstream before regenerating: "
            f"missing={sorted(EXPECTED_WRITE_OPERATION_IDS - writes)}, "
            f"new={sorted(writes - EXPECTED_WRITE_OPERATION_IDS)}"
        )
    if len(writes) != EXPECTED_WRITES:
        fail(f"expected {EXPECTED_WRITES} writes, found {len(writes)}")
    if len(operations) != EXPECTED_READS:
        fail(f"expected {EXPECTED_READS} reads, found {len(operations)}")
    if counts != Counter(market=EXPECTED_MARKET_READS, account=EXPECTED_ACCOUNT_READS):
        fail(f"unexpected read scope counts: {dict(counts)}")
    if len(operation_ids) != len(set(operation_ids)):
        fail("duplicate operation IDs")
    if writes.intersection(operation_ids):
        fail("a POST operation escaped into the read catalog")
    if any(operation["http_method"] != "GET" for operation in operations):
        fail("the read catalog contains a non-GET operation")

    return {
        "schema": SCHEMA,
        "upstream": {
            "repository": UPSTREAM_REPOSITORY,
            "commit": UPSTREAM_COMMIT,
            "config_root": "MCP/Kis Trading MCP/configs",
            "examples_root": "examples_llm",
        },
        "policy": {
            "fixed_hosts": {
                "real": "https://openapi.koreainvestment.com:9443",
                "demo": "https://openapivts.koreainvestment.com:29443",
            },
            "http_methods": ["GET"],
            "orders_included": False,
            "account_reads_available_in_v1": True,
        },
        "counts": {
            "read_operations": len(operations),
            "market_reads": counts["market"],
            "account_reads": counts["account"],
            "excluded_writes": len(writes),
        },
        "excluded_write_operation_ids": sorted(writes),
        "operations": operations,
    }


def encoded(manifest: dict[str, Any]) -> bytes:
    return (json.dumps(manifest, ensure_ascii=False, indent=2) + "\n").encode("utf-8")


def parse_args() -> argparse.Namespace:
    app_root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--source",
        type=Path,
        required=True,
        help="Pinned koreainvestment/open-trading-api checkout",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=app_root / "marketplace" / "kis-read-api-v1.json",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="Validate that --output exactly matches a fresh deterministic build",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        payload = encoded(build_manifest(args.source.resolve()))
        if args.check:
            if not args.output.is_file():
                print(f"missing generated manifest: {args.output}", file=sys.stderr)
                return 1
            if args.output.read_bytes() != payload:
                print(
                    f"stale generated manifest: run {Path(__file__).name} "
                    f"--source {args.source}",
                    file=sys.stderr,
                )
                return 1
        else:
            args.output.write_bytes(payload)
        manifest = json.loads(payload)
        counts = manifest["counts"]
        print(
            "KIS read API manifest OK: "
            f"{counts['read_operations']} reads "
            f"({counts['market_reads']} market, {counts['account_reads']} account), "
            f"{counts['excluded_writes']} writes excluded"
        )
        return 0
    except (
        ContractError,
        OSError,
        subprocess.CalledProcessError,
        json.JSONDecodeError,
    ) as error:
        print(f"KIS manifest generation failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
