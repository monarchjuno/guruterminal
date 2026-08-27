import assert from "node:assert/strict";
import {
  constants,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import net from "node:net";
import { randomUUID } from "node:crypto";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import guruTerminalExtension, {
  hostContextOpenNoFollowForPlatform,
  resolveCapabilityComponent,
} from "./guruterminal-extension.mjs";

const EXPECTED_TOOLS = [
  "artifact_list",
  "artifact_publish",
  "artifact_read",
  "capability_load",
  "capability_search",
  "chart_publish",
  "chart_query",
  "compute_run",
  "decision_submit",
  "edit",
  "evidence_create",
  "finance_calculate",
  "finance_company_data",
  "finance_filings",
  "finance_macro_data",
  "finance_market_data",
  "finance_resolve_entity",
  "finance_sources",
  "find",
  "grep",
  "ls",
  "memory_patch_propose",
  "memory_previous",
  "memory_read",
  "memory_search",
  "read",
  "run_results_list",
  "web_fetch",
  "web_search",
  "write",
];

test("keeps host-context nofollow protection off Windows only", () => {
  assert.equal(hostContextOpenNoFollowForPlatform("win32"), 0);
  assert.equal(
    hostContextOpenNoFollowForPlatform("darwin"),
    constants.O_NOFOLLOW ?? 0,
  );
});

test("resolves only canonical or unambiguous non-MCP capability identifiers", () => {
  const direct = { id: "direct", kind: "tool", provider_ids: [] };
  const providerAlias = {
    id: "macro-data",
    kind: "tool",
    provider_ids: ["world-bank.indicators"],
  };
  const directCollision = {
    id: "different-tool",
    kind: "tool",
    provider_ids: ["direct"],
  };
  const firstShared = { id: "first", kind: "tool", provider_ids: ["shared"] };
  const secondShared = { id: "second", kind: "tool", provider_ids: ["shared"] };
  const mcp = { id: "mcp/openbb", kind: "mcp", provider_ids: ["yfinance"] };
  const components = new Map([
    [direct.id, direct],
    [providerAlias.id, providerAlias],
    [directCollision.id, directCollision],
    [firstShared.id, firstShared],
    [secondShared.id, secondShared],
    [mcp.id, mcp],
  ]);

  assert.equal(resolveCapabilityComponent(components, "direct"), direct);
  assert.equal(
    resolveCapabilityComponent(components, "world-bank.indicators"),
    providerAlias,
  );
  assert.equal(resolveCapabilityComponent(components, "shared"), undefined);
  assert.equal(resolveCapabilityComponent(components, "yfinance"), undefined);
  assert.equal(resolveCapabilityComponent(components, "unknown"), undefined);
});

const BUNDLED_COMPONENTS = [
  {
    id: "guruterminal.workbench/authoring",
    kind: "tool",
    name: "Workbench authoring",
    description: "Create or edit files in the bounded workbench.",
    tool_names: ["write", "edit"],
    provider_ids: [],
  },
  {
    id: "guruterminal.artifacts/markdown-publishing",
    kind: "tool",
    name: "Markdown artifact publishing",
    description: "Publish or revise a Markdown artifact.",
    tool_names: ["artifact_publish"],
    provider_ids: [],
  },
  {
    id: "guruterminal.memory/evidence-and-decisions",
    kind: "tool",
    name: "Evidence and decisions",
    description: "Create canonical Evidence or submit an explicit Decision.",
    tool_names: ["evidence_create", "decision_submit"],
    provider_ids: [],
  },
  {
    id: "guruterminal.memory/learning",
    kind: "tool",
    name: "Memory learning",
    description: "Propose a complete Wiki or Lens record when enabled.",
    tool_names: ["memory_patch_propose"],
    provider_ids: [],
  },
  {
    id: "guruterminal.charting/authoring",
    kind: "tool",
    name: "Chart authoring",
    description: "Publish charts with indicators and drawings.",
    tool_names: ["chart_query", "chart_publish"],
    provider_ids: [],
  },
  {
    id: "guruterminal.compute-python/python",
    kind: "tool",
    name: "Sandboxed compute",
    description: "Run bounded offline Python or JavaScript analysis.",
    tool_names: ["compute_run"],
    provider_ids: ["guruterminal.compute-python"],
  },
  {
    id: "guruterminal.finance-core/source-catalog",
    kind: "tool",
    name: "Finance source catalog",
    description: "Inspect installed finance sources.",
    tool_names: ["finance_sources"],
    provider_ids: ["guruterminal.finance-core"],
  },
  {
    id: "guruterminal.finance-core/calculations",
    kind: "tool",
    name: "Finance calculations",
    description: "Run deterministic finance calculations.",
    tool_names: ["finance_calculate"],
    provider_ids: ["guruterminal.finance-core"],
  },
  {
    id: "guruterminal.finance-providers/macro-data",
    kind: "tool",
    name: "Structured macro data",
    description: "Fetch macro series from enabled providers: world-bank.indicators.",
    tool_names: ["finance_macro_data"],
    provider_ids: ["world-bank.indicators"],
  },
  {
    id: "guruterminal.finance-providers/market-data",
    kind: "tool",
    name: "Structured market data",
    description: "Fetch market history from enabled native providers.",
    tool_names: ["finance_market_data"],
    provider_ids: ["krx.market-data", "koreainvestment.market-data"],
  },
  {
    id: "mcp/openbb",
    kind: "mcp",
    server_id: "openbb",
    name: "OpenBB providers",
    description: "Discover and activate read-only OpenBB finance tools.",
    tool_names: [],
    provider_ids: ["alpha_vantage", "fred", "sec", "yfinance"],
  },
  {
    id: "guruterminal.finance-providers/company-disclosures",
    kind: "tool",
    name: "Official company data and filings",
    description: "Fetch company facts and exact filings from OpenDART.",
    tool_names: ["finance_company_data", "finance_filings", "finance_resolve_entity"],
    provider_ids: ["opendart.disclosures"],
  },
  {
    id: "community.web-research/research",
    kind: "tool",
    name: "Public web research",
    description: "Search the public web and materialize exact source pages.",
    tool_names: ["web_search", "web_fetch"],
    provider_ids: ["community.web-research"],
  },
];

function runtimeContext(mode, coreToolNames, components = [], extra = {}) {
  return {
    agent_runtime: {
      schema: "guruterminal-agent-runtime/1",
      mode,
      core_tool_names: coreToolNames,
      components,
    },
    ...extra,
  };
}

function compactHostContext(parsed) {
  return JSON.stringify({
    ...parsed,
    agent_runtime: {
      schema: parsed.agent_runtime.schema,
      mode: parsed.agent_runtime.mode,
      capability_ids: parsed.agent_runtime.capability_ids,
      core_tool_names: parsed.agent_runtime.core_tool_names,
      components: parsed.agent_runtime.components.map((component) => ({
        id: component.id,
        ...(component.kind === "mcp"
          ? { kind: component.kind, server_id: component.server_id }
          : {}),
        name: component.name,
        description: component.description,
        tool_names: component.tool_names,
        provider_ids: component.provider_ids ?? [],
      })),
    },
  });
}

async function startToolBroker(t, handler) {
  const temporary = mkdtempSync(join(tmpdir(), "guruterminal-extension-broker-"));
  const socketPath =
    process.platform === "win32"
      ? `\\\\.\\pipe\\guruterminal-extension-broker-${randomUUID()}`
      : join(temporary, "broker.sock");
  const previousSocket = process.env.GURUTERMINAL_BROKER_SOCKET;
  const previousToken = process.env.GURUTERMINAL_BROKER_TOKEN;
  process.env.GURUTERMINAL_BROKER_SOCKET = socketPath;
  process.env.GURUTERMINAL_BROKER_TOKEN = "workbench-test-token";
  const server = net.createServer((socket) => {
    let buffered = "";
    let requestId;
    socket.setEncoding("utf8");
    socket.on("data", (chunk) => {
      buffered += chunk;
      while (true) {
        const newline = buffered.indexOf("\n");
        if (newline < 0) return;
        const frame = JSON.parse(buffered.slice(0, newline));
        buffered = buffered.slice(newline + 1);
        if (requestId) {
          assert.deepEqual(frame, {
            protocol: "guruterminal-tool/1",
            id: requestId,
            delivered: true,
          });
          socket.end(`${JSON.stringify({
            protocol: "guruterminal-tool/1",
            id: requestId,
            committed: true,
          })}\n`);
          return;
        }
        requestId = frame.id;
        try {
          const result = handler(frame);
          socket.write(`${JSON.stringify({
            protocol: "guruterminal-tool/1",
            id: frame.id,
            ok: true,
            result,
          })}\n`);
        } catch (error) {
          socket.write(`${JSON.stringify({
            protocol: "guruterminal-tool/1",
            id: frame.id,
            ok: false,
            error: { message: error.message },
          })}\n`);
        }
      }
    });
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(socketPath, resolve);
  });
  t.after(async () => {
    await new Promise((resolve) => server.close(resolve));
    if (previousSocket === undefined) delete process.env.GURUTERMINAL_BROKER_SOCKET;
    else process.env.GURUTERMINAL_BROKER_SOCKET = previousSocket;
    if (previousToken === undefined) delete process.env.GURUTERMINAL_BROKER_TOKEN;
    else process.env.GURUTERMINAL_BROKER_TOKEN = previousToken;
    rmSync(temporary, { recursive: true, force: true });
  });
}

function workbenchBrokerHandler() {
  return (request) => {
    const queryMethod = ["workbench.ls", "workbench.find", "workbench.grep"]
      .includes(request.method);
    const path = request.params?.path ?? (queryMethod ? "." : undefined);
    if (typeof path !== "string" || path.includes("\0")) {
      throw new Error("Workbench path is invalid");
    }
    if (path.includes("..") || path.startsWith("/") || /^[A-Za-z]:[\\/]/.test(path)) {
      throw new Error("Path is outside this Guru's workbench");
    }
    const attachment = path === "attachments" || path.startsWith("attachments/");
    if (attachment && ["workbench.write", "workbench.edit"].includes(request.method)) {
      throw new Error("App-owned attachment snapshots are read-only");
    }
    if (request.method === "workbench.write") {
      return {
        status: "ok",
        path,
        bytes: Buffer.byteLength(request.params.content ?? "", "utf8"),
        revision: "a".repeat(64),
      };
    }
    if (request.method === "workbench.read") {
      return {
        path,
        content: attachment ? "immutable attachment" : "durable insight",
        total_lines: 1,
        revision: "a".repeat(64),
        result_ref: "result:workbench-read",
      };
    }
    if (request.method === "workbench.edit") {
      throw new Error("App-owned attachment snapshots are read-only");
    }
    if (request.method === "workbench.ls") {
      return {
        text: "dir \tnotes",
        count: 1,
        truncated: false,
        result_ref: "result:workbench-ls",
      };
    }
    if (request.method === "workbench.find") {
      return {
        text: "notes/alpha.md",
        count: 1,
        truncated: false,
        result_ref: "result:workbench-find",
      };
    }
    if (request.method === "workbench.grep") {
      return {
        text: "notes/alpha.md-1-one\nnotes/alpha.md:2:match here\nnotes/alpha.md-3-three\n\n[Skipped 1 binary file: notes/data.bin]",
        count: 1,
        skipped_binary: 1,
        skipped_binary_paths: ["notes/data.bin"],
        truncated: false,
        warnings: ["Skipped 1 binary file: notes/data.bin"],
        result_ref: "result:workbench-grep",
      };
    }
    throw new Error(`unexpected method ${request.method}`);
  };
}

test("registers only the product allowlist and round-trips through the private broker", async (t) => {
  const temporary = mkdtempSync(join(tmpdir(), "guruterminal-extension-"));
  const socketPath =
    process.platform === "win32"
      ? `\\\\.\\pipe\\guruterminal-extension-${randomUUID()}`
      : join(temporary, "broker.sock");
  const hostContextPath = join(temporary, "host-context.json");
  const hostContext = JSON.stringify({
    agent_harness: { mode: "chat" },
    agent_runtime: runtimeContext(
      "chat",
      [
        "read", "ls", "find", "grep",
        "run_results_list",
        "memory_search", "memory_read", "memory_previous", "capability_search", "capability_load",
        "artifact_list", "artifact_read",
      ],
      BUNDLED_COMPONENTS,
    ).agent_runtime,
  });
  writeFileSync(hostContextPath, hostContext, { mode: 0o600 });
  const previousSocket = process.env.GURUTERMINAL_BROKER_SOCKET;
  const previousToken = process.env.GURUTERMINAL_BROKER_TOKEN;
  const previousHostContext = process.env.GURUTERMINAL_HOST_CONTEXT_FILE;
  const previousSkillFiles = process.env.GURUTERMINAL_SKILL_FILES;
  process.env.GURUTERMINAL_BROKER_SOCKET = socketPath;
  process.env.GURUTERMINAL_BROKER_TOKEN = "one-turn-capability";
  process.env.GURUTERMINAL_HOST_CONTEXT_FILE = hostContextPath;
  process.env.GURUTERMINAL_SKILL_FILES = "[]";
  process.env.GURUTERMINAL_MODEL_RUN_OPTIONS = JSON.stringify({
    performance: "fast",
  });

  const registered = [];
  const handlers = new Map();
  const activeToolSnapshots = [];
  guruTerminalExtension({
    registerTool: (tool) => registered.push(tool),
    on: (name, handler) => handlers.set(name, handler),
    setActiveTools: (names) => activeToolSnapshots.push(names),
  });
  assert.deepEqual(
    registered.map((tool) => tool.name).sort(),
    EXPECTED_TOOLS,
  );
  assert.deepEqual(
    registered.find((tool) => tool.name === "artifact_read")?.parameters.required,
    ["artifact_id"],
  );
  assert.equal(existsSync(hostContextPath), false);
  assert.equal(process.env.GURUTERMINAL_HOST_CONTEXT_FILE, undefined);
  assert.equal(process.env.GURUTERMINAL_MODEL_RUN_OPTIONS, undefined);
  await handlers.get("session_start")();
  const injected = await handlers.get("before_agent_start")({ systemPrompt: "base" });
  assert.equal(
    injected.systemPrompt,
    `base\n\n${compactHostContext(JSON.parse(hostContext))}`,
  );
  assert(injected.systemPrompt.includes("\"tool_names\""));
  assert(!injected.systemPrompt.includes("\"parameters\""));
  assert(injected.systemPrompt.includes("mcp/openbb"));
  assert(injected.systemPrompt.includes("\"server_id\":\"openbb\""));
  assert.deepEqual(
    handlers.get("before_provider_request")(
      { payload: { model: "gpt-test" } },
      { model: { api: "openai-codex-responses" } },
    ),
    { model: "gpt-test", service_tier: "priority" },
  );
  assert(!activeToolSnapshots[0].includes("compute_run"));
  assert(activeToolSnapshots[0].includes("capability_search"));
  for (const name of [
    "write",
    "edit",
    "artifact_publish",
    "decision_submit",
    "evidence_create",
    "memory_patch_propose",
  ]) {
    assert(!activeToolSnapshots[0].includes(name), `${name} must be deferred`);
  }
  const cardBytes = (tools) => Buffer.byteLength(JSON.stringify(
    tools.map(({ name, description, parameters }) => ({ name, description, parameters })),
  ));
  const eagerTools = new Set(activeToolSnapshots[0]);
  const eagerSchemaBytes = cardBytes(registered.filter((tool) => eagerTools.has(tool.name)));
  const registeredSchemaBytes = cardBytes(registered);
  assert(eagerSchemaBytes < registeredSchemaBytes);
  assert(cardBytes(registered.filter((tool) => [
    "write",
    "edit",
    "artifact_publish",
    "decision_submit",
    "evidence_create",
    "memory_patch_propose",
  ].includes(tool.name))) > 0);

  let resolveReceived;
  let rejectReceived;
  const received = new Promise((resolve, reject) => {
    resolveReceived = resolve;
    rejectReceived = reject;
  });
  const server = net.createServer((socket) => {
    let buffered = "";
    let requestId;
    socket.setEncoding("utf8");
    socket.on("data", (chunk) => {
      buffered += chunk;
      const newline = buffered.indexOf("\n");
      if (newline < 0) return;
      try {
        const request = JSON.parse(buffered.slice(0, newline));
        buffered = buffered.slice(newline + 1);
        if (requestId) {
          assert.deepEqual(request, {
            protocol: "guruterminal-tool/1",
            id: requestId,
            delivered: true,
          });
          socket.end(`${JSON.stringify({
            protocol: "guruterminal-tool/1",
            id: requestId,
            committed: true,
          })}\n`);
          resolveReceived();
          return;
        }
        assert.equal(request.protocol, "guruterminal-tool/1");
        assert.equal(request.token, "one-turn-capability");
        assert.equal(request.method, "guru.search");
        assert.deepEqual(request.params, { query: "margin" });
        requestId = request.id;
        socket.write(`${JSON.stringify({
          protocol: "guruterminal-tool/1",
          id: request.id,
          ok: true,
          result: { records: ["lens-quality"] },
        })}\n`);
      } catch (error) {
        rejectReceived(error);
        socket.destroy();
      }
    });
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(socketPath, resolve);
  });

  t.after(async () => {
    await new Promise((resolve) => server.close(resolve));
    if (previousSocket === undefined) delete process.env.GURUTERMINAL_BROKER_SOCKET;
    else process.env.GURUTERMINAL_BROKER_SOCKET = previousSocket;
    if (previousToken === undefined) delete process.env.GURUTERMINAL_BROKER_TOKEN;
    else process.env.GURUTERMINAL_BROKER_TOKEN = previousToken;
    if (previousHostContext === undefined) delete process.env.GURUTERMINAL_HOST_CONTEXT_FILE;
    else process.env.GURUTERMINAL_HOST_CONTEXT_FILE = previousHostContext;
    if (previousSkillFiles === undefined) delete process.env.GURUTERMINAL_SKILL_FILES;
    else process.env.GURUTERMINAL_SKILL_FILES = previousSkillFiles;
    rmSync(temporary, { recursive: true, force: true });
  });

  const search = registered.find((tool) => tool.name === "memory_search");
  const calculate = registered.find((tool) => tool.name === "finance_calculate");
  const compute = registered.find((tool) => tool.name === "compute_run");
  const webSearch = registered.find((tool) => tool.name === "web_search");
  const webFetch = registered.find((tool) => tool.name === "web_fetch");
  const macroData = registered.find((tool) => tool.name === "finance_macro_data");
  const marketData = registered.find((tool) => tool.name === "finance_market_data");
  const companyData = registered.find((tool) => tool.name === "finance_company_data");
  const filings = registered.find((tool) => tool.name === "finance_filings");
  const evidenceCreate = registered.find((tool) => tool.name === "evidence_create");
  const memoryProposal = registered.find((tool) => tool.name === "memory_patch_propose");
  const chartPublish = registered.find((tool) => tool.name === "chart_publish");
  assert.equal(webFetch.parameters.required, undefined);
  assert.ok(webFetch.parameters.properties.url);
  assert.deepEqual(webFetch.parameters.properties.offset, {
    type: "integer",
    minimum: 0,
    maximum: 2097151,
  });
  assert.ok(webSearch.parameters.properties.recency);
  assert.ok(webSearch.parameters.properties.include_domains);
  assert.equal(webSearch.parameters.properties.query.maxLength, 4096);
  assert.equal(webSearch.parameters.properties.provider, undefined);
  assert.match(marketData.description, /Order, correction, and cancellation operations are absent/);
  assert.match(compute.description, /no network, host files, environment, subprocess/);
  assert.deepEqual(evidenceCreate.parameters.required, [
    "title",
    "summary",
    "as_of",
    "markdown",
    "citations",
  ]);
  assert.equal(evidenceCreate.parameters.properties.citations.maxItems, 16);
  assert.deepEqual(evidenceCreate.parameters.properties.citations.items.required, ["result_ref"]);
  assert.equal(evidenceCreate.parameters.properties.citations.items.properties.pointer, undefined);
  assert.equal(memoryProposal.parameters.properties.target_id.pattern, "^(?:wiki|lens):[^\\s]+$");
  assert.equal(chartPublish.parameters.properties.source_ref, undefined);
  assert.deepEqual(chartPublish.parameters.then.required, ["mode", "title", "dataset", "view"]);
  const [fromResultDataset, inlineDataset] = chartPublish.parameters.properties.dataset.oneOf;
  assert.deepEqual(fromResultDataset.required, ["from_result"]);
  assert.deepEqual(
    fromResultDataset.properties.from_result.required,
    ["result_ref", "rows_pointer", "columns"],
  );
  assert.deepEqual(
    fromResultDataset.properties.from_result.properties.columns.items.required,
    ["id", "label", "kind", "pointer"],
  );
  assert.deepEqual(inlineDataset.required, ["inline"]);
  assert.deepEqual(inlineDataset.properties.inline.required, ["columns", "rows"]);
  assert.equal(inlineDataset.properties.inline.properties.upstream_result_refs.uniqueItems, true);
  assert.equal(chartPublish.parameters.properties.drawings.maxItems, 32);
  const drawingKinds = chartPublish.parameters.properties.drawings.items.properties.kind.enum;
  for (const kind of [
    "annotation",
    "rectangle",
    "arrow",
    "measure",
    "fibonacci_extension",
    "long_position",
    "short_position",
    "segment",
  ]) {
    assert(drawingKinds.includes(kind), `missing drawing kind ${kind}`);
  }
  assert.equal(chartPublish.parameters.properties.drawings.items.properties.label.maxLength, 80);
  assert(chartPublish.parameters.properties.studies.items.properties.module_id.enum.includes("SAR"));
  assert(chartPublish.parameters.properties.studies.items.properties.module_id.enum.includes("WR"));
  assert.deepEqual(memoryProposal.parameters.required, [
    "kind",
    "target_id",
    "proposed_markdown",
    "rationale",
    "source_ids",
  ]);
  assert.equal(memoryProposal.parameters.properties.source_ids.maxItems, 32);
  assert.equal(memoryProposal.parameters.properties.source_ids.uniqueItems, true);
  assert.deepEqual(compute.parameters.required, ["language", "source"]);
  assert.deepEqual(compute.parameters.allOf[0].then.properties.packages, {
    type: "array",
    maxItems: 0,
  });
  assert.deepEqual(compute.parameters.properties.language.enum, ["python", "javascript"]);
  assert.deepEqual(compute.parameters.properties.packages.items.enum, [
    "numpy",
    "pandas",
    "scipy",
    "statsmodels",
    "scikit-learn",
  ]);
  assert.deepEqual(calculate.parameters.required, ["operations"]);
  assert.equal(calculate.parameters.properties.operations.minItems, 1);
  assert.equal(calculate.parameters.properties.operations.maxItems, 64);
  const calculateOperation = calculate.parameters.properties.operations.items;
  assert.deepEqual(calculateOperation.required, ["id", "operation", "arguments"]);
  assert.equal(calculateOperation.properties.id.minLength, 1);
  assert.equal(calculateOperation.properties.id.maxLength, 64);
  assert.deepEqual(calculateOperation.properties.operation.enum, [
    "compound_annual_growth_rate",
    "currency_convert",
    "discounted_cash_flow",
    "dcf_sensitivity",
    "enterprise_value_bridge",
    "internal_rate_of_return",
    "percentage_change",
    "period_aggregate",
    "point_in_time_filter",
    "ratio",
    "risk_metrics",
    "series_statistics",
    "weighted_average_cost_of_capital",
  ]);
  const calculateArguments = (operation) => {
    const branch = calculateOperation.allOf.find(
      (item) => item.if?.properties?.operation?.const === operation,
    );
    assert.ok(branch, `missing finance_calculate branch for ${operation}`);
    return branch.then.properties.arguments;
  };
  assert.equal(
    calculateOperation.allOf.length,
    calculateOperation.properties.operation.enum.length,
  );
  const percentageChange = calculateArguments("percentage_change");
  assert.equal(percentageChange.additionalProperties, false);
  assert.deepEqual(percentageChange.required, ["start", "end"]);
  assert.equal(percentageChange.properties.source_ref, undefined);
  assert.equal(percentageChange.properties.market_source_ref, undefined);
  assert.equal(percentageChange.properties.field, undefined);
  assert.equal(percentageChange.properties.market_field, undefined);
  assert.equal(percentageChange.properties.unit, undefined);
  const ratio = calculateArguments("ratio");
  assert.ok(ratio.properties.unit);
  assert.deepEqual(ratio.required, ["numerator", "denominator"]);
  assert.equal(calculateArguments("series_statistics").properties.unit, undefined);
  assert.deepEqual(macroData.parameters.properties.provider.enum, ["world-bank.indicators"]);
  assert.deepEqual(
    marketData.parameters.oneOf.map((branch) => branch.properties.provider.enum[0]),
    ["krx.market-data", "koreainvestment.market-data"],
  );
  const kisBranch = marketData.parameters.oneOf.at(-1);
  assert.deepEqual(kisBranch.required, ["provider", "operation_id", "params"]);
  assert.equal(kisBranch.properties.params.maxProperties, 64);
  assert.equal(kisBranch.properties.params.additionalProperties.type, "string");
  assert(!Object.keys(kisBranch.properties).some((name) => /account|credential|app.?key|secret/i.test(name)));
  assert.deepEqual(companyData.parameters.properties.provider.enum, ["opendart.disclosures"]);
  assert.deepEqual(
    filings.parameters.oneOf.map((branch) => [
      branch.properties.provider.enum[0],
      branch.properties.operation.enum[0],
    ]),
    [
      ["opendart.disclosures", "search"],
      ["opendart.disclosures", "read"],
    ],
  );
  for (const tool of [marketData, filings]) {
    for (const branch of tool.parameters.oneOf) {
      assert.equal(branch.type, "object");
      assert.equal(branch.additionalProperties, false);
      assert(!Object.keys(branch.properties).some((name) => /api.?key|credential|contact/i.test(name)));
    }
  }
  for (const tool of [macroData, companyData]) {
    assert.equal(tool.parameters.type, "object");
    assert.equal(tool.parameters.additionalProperties, false);
    assert(!Object.keys(tool.parameters.properties).some((name) => /api.?key|credential|contact/i.test(name)));
  }
  const result = await search.execute(
    "tool-call",
    { query: "margin" },
    new AbortController().signal,
  );
  await received;
  assert.deepEqual(result.details, { records: ["lens-quality"] });
  assert.equal(result.content[0].type, "text");

  const capabilitySearch = registered.find((tool) => tool.name === "capability_search");
  const capabilityLoad = registered.find((tool) => tool.name === "capability_load");
  assert.deepEqual(capabilitySearch.parameters.required, ["query"]);
  assert.equal(capabilitySearch.parameters.properties.query.minLength, 1);
  await assert.rejects(
    () => capabilitySearch.execute("discover-empty", { query: "   " }),
    /must not be empty/,
  );
  const publishing = await capabilitySearch.execute("discover-publishing", {
    query: "publish",
  });
  assert.deepEqual(
    publishing.details.components.map((component) => component.id),
    [
      "guruterminal.artifacts/markdown-publishing",
      "guruterminal.charting/authoring",
    ],
  );
  const workbench = await capabilitySearch.execute("discover-workbench", {
    query: "workbench",
  });
  assert.deepEqual(workbench.details.components[0].tools.map((tool) => tool.name), [
    "write",
    "edit",
  ]);
  await capabilityLoad.execute("load-workbench", {
    id: "guruterminal.workbench/authoring",
  });
  assert(activeToolSnapshots.at(-1).includes("write"));
  assert(activeToolSnapshots.at(-1).includes("edit"));
  assert(!activeToolSnapshots.at(-1).includes("evidence_create"));
  await capabilityLoad.execute("load-evidence", {
    id: "guruterminal.memory/evidence-and-decisions",
  });
  assert(activeToolSnapshots.at(-1).includes("evidence_create"));
  assert(activeToolSnapshots.at(-1).includes("decision_submit"));
  assert(!activeToolSnapshots.at(-1).includes("memory_patch_propose"));
  await capabilityLoad.execute("load-learning", {
    id: "guruterminal.memory/learning",
  });
  assert(activeToolSnapshots.at(-1).includes("memory_patch_propose"));
  const discovered = await capabilitySearch.execute("discover", { query: "python" });
  assert.deepEqual(discovered.details.components.map((component) => component.id), [
    "guruterminal.compute-python/python",
  ]);
  const discoveredJs = await capabilitySearch.execute("discover-js", { query: "javascript" });
  assert.deepEqual(discoveredJs.details.components.map((component) => component.id), [
    "guruterminal.compute-python/python",
  ]);
  assert.equal(discovered.details.components[0].loaded, false);
  assert(Array.isArray(discovered.details.components[0].tools));
  assert.equal(discovered.details.components[0].tools[0].name, "compute_run");
  assert.equal(typeof discovered.details.components[0].tools[0].parameters, "object");
  const discoveredOpenbb = await capabilitySearch.execute("discover-openbb", {
    query: "openbb",
  });
  assert.deepEqual(discoveredOpenbb.details.components.map((component) => component.id), [
    "mcp/openbb",
  ]);
  assert.equal(discoveredOpenbb.details.components[0].loaded, false);
  assert.deepEqual(discoveredOpenbb.details.components[0].provider_ids, [
    "alpha_vantage", "fred", "sec", "yfinance",
  ]);
  assert.deepEqual(discoveredOpenbb.details.components[0].tools, []);
  const loadedWeb = await capabilityLoad.execute("load-web", {
    id: "community.web-research/research",
  });
  assert.deepEqual(loadedWeb.details.tool_names, ["web_search", "web_fetch"]);
  assert(activeToolSnapshots.at(-1).includes("web_search"));
  assert(activeToolSnapshots.at(-1).includes("web_fetch"));
  const loadedMacro = await capabilityLoad.execute("load-macro-provider", {
    id: "world-bank.indicators",
  });
  assert.deepEqual(loadedMacro.details, {
    id: "guruterminal.finance-providers/macro-data",
    kind: "tool",
    name: "Structured macro data",
    tool_names: ["finance_macro_data"],
  });
  assert(activeToolSnapshots.at(-1).includes("finance_macro_data"));
  const activeAfterMacro = activeToolSnapshots.at(-1);
  await assert.rejects(
    () => capabilityLoad.execute("load-ambiguous", { id: "guruterminal.finance-core" }),
    /not available in this run/,
  );
  await assert.rejects(
    () => capabilityLoad.execute("load-mcp-provider", { id: "yfinance" }),
    /not available in this run/,
  );
  assert.deepEqual(activeToolSnapshots.at(-1), activeAfterMacro);
  await assert.rejects(
    () => capabilityLoad.execute("load-missing", { id: "unapproved/tool" }),
    /not available in this run/,
  );
});

test("loads namespaced MCP tools and reconciles list changes", async (t) => {
  const temporary = mkdtempSync(join(tmpdir(), "guruterminal-extension-mcp-"));
  const hostContextPath = join(temporary, "host-context.json");
  const previousHostContext = process.env.GURUTERMINAL_HOST_CONTEXT_FILE;
  const previousSkillFiles = process.env.GURUTERMINAL_SKILL_FILES;
  const previousRunOptions = process.env.GURUTERMINAL_MODEL_RUN_OPTIONS;
  const component = {
    id: "mcp/openbb",
    kind: "mcp",
    server_id: "openbb",
    name: "OpenBB providers",
    description: "Read-only OpenBB financial data.",
    tool_names: [],
    provider_ids: ["fmp", "yfinance"],
  };
  writeFileSync(
    hostContextPath,
    JSON.stringify(runtimeContext("chat", ["capability_search", "capability_load"], [component])),
    { mode: 0o600 },
  );
  process.env.GURUTERMINAL_HOST_CONTEXT_FILE = hostContextPath;
  process.env.GURUTERMINAL_SKILL_FILES = "[]";
  process.env.GURUTERMINAL_MODEL_RUN_OPTIONS = "{}";

  const activateCard = {
    name: "mcp__openbb__activate_tools",
    mcp_name: "activate_tools",
    server_id: "openbb",
    label: "Activate tools",
    description: "Activate an allowlisted read-only tool.",
    parameters: {
      type: "object",
      additionalProperties: false,
      required: ["tool_names"],
      properties: {
        tool_names: { type: "array", items: { type: "string" } },
      },
    },
  };
  const quoteCard = {
    name: "mcp__openbb__equity_price_quote",
    mcp_name: "equity_price_quote",
    server_id: "openbb",
    label: "Equity quote",
    description: "Read an equity quote.",
    parameters: {
      type: "object",
      additionalProperties: false,
      required: ["symbol", "provider"],
      properties: {
        symbol: { type: "string" },
        provider: { type: "string", enum: ["fmp", "yfinance"] },
      },
    },
  };
  const calls = [];
  let quoteCalls = 0;
  await startToolBroker(t, (request) => {
    calls.push({ method: request.method, params: request.params });
    if (request.method === "mcp.connect") {
      return { server_id: "openbb", tools: [activateCard] };
    }
    if (request.method === "mcp.call" && request.params.tool_name === "activate_tools") {
      return {
        result: { content: [{ type: "text", text: "Activated: equity_price_quote" }] },
        tools: [activateCard, quoteCard],
      };
    }
    if (request.method === "mcp.call" && request.params.tool_name === "equity_price_quote") {
      quoteCalls += 1;
      if (quoteCalls === 2) {
        return {
          call_error: true,
          tools: [activateCard],
        };
      }
      if (quoteCalls === 3) {
        return {
          call_error: true,
          session_stopped: true,
        };
      }
      return {
        result: {
          content: [{ type: "text", text: "{\"price\":213.4,\"provider\":\"fmp\"}" }],
          structuredContent: { price: 213.4, provider: "fmp" },
          result_ref: "result:delivered",
        },
      };
    }
    throw new Error(`unexpected method ${request.method}`);
  });
  t.after(() => {
    if (previousHostContext === undefined) delete process.env.GURUTERMINAL_HOST_CONTEXT_FILE;
    else process.env.GURUTERMINAL_HOST_CONTEXT_FILE = previousHostContext;
    if (previousSkillFiles === undefined) delete process.env.GURUTERMINAL_SKILL_FILES;
    else process.env.GURUTERMINAL_SKILL_FILES = previousSkillFiles;
    if (previousRunOptions === undefined) delete process.env.GURUTERMINAL_MODEL_RUN_OPTIONS;
    else process.env.GURUTERMINAL_MODEL_RUN_OPTIONS = previousRunOptions;
    rmSync(temporary, { recursive: true, force: true });
  });

  const registered = [];
  const activeToolSnapshots = [];
  guruTerminalExtension({
    registerTool: (tool) => registered.push(tool),
    on: () => {},
    setActiveTools: (names) => activeToolSnapshots.push(names),
  });
  const capabilityLoad = registered.find((tool) => tool.name === "capability_load");
  const loaded = await capabilityLoad.execute("load-openbb", { id: "mcp/openbb" });
  assert.deepEqual(loaded.details.tool_names, ["mcp__openbb__activate_tools"]);
  assert(activeToolSnapshots.at(-1).includes("mcp__openbb__activate_tools"));

  const activate = registered.find((tool) => tool.name === "mcp__openbb__activate_tools");
  await activate.execute("activate-quote", { tool_names: ["equity_price_quote"] });
  const quote = registered.find((tool) => tool.name === "mcp__openbb__equity_price_quote");
  assert.ok(quote);
  assert(activeToolSnapshots.at(-1).includes("mcp__openbb__equity_price_quote"));
  const quoted = await quote.execute("read-quote", { symbol: "AAPL", provider: "fmp" });
  assert.equal(quoted.details.result_ref, "result:delivered");
  await assert.rejects(
    () => quote.execute("stale-quote", { symbol: "AAPL", provider: "fmp" }),
    /Bundled MCP tool call failed/,
  );
  assert(!activeToolSnapshots.at(-1).includes("mcp__openbb__equity_price_quote"));
  assert(activeToolSnapshots.at(-1).includes("mcp__openbb__activate_tools"));
  await activate.execute("reactivate-quote", { tool_names: ["equity_price_quote"] });
  assert(activeToolSnapshots.at(-1).includes("mcp__openbb__equity_price_quote"));
  await assert.rejects(
    () => quote.execute("stopped-runtime", { symbol: "AAPL", provider: "fmp" }),
    /Bundled MCP runtime stopped/,
  );
  assert(!activeToolSnapshots.at(-1).includes("mcp__openbb__activate_tools"));
  assert(!activeToolSnapshots.at(-1).includes("mcp__openbb__equity_price_quote"));
  assert.deepEqual(calls.map((call) => call.method), [
    "mcp.connect",
    "mcp.call",
    "mcp.call",
    "mcp.call",
    "mcp.call",
    "mcp.call",
  ]);
  assert.deepEqual(calls.at(-1).params, {
    server_id: "openbb",
    tool_name: "equity_price_quote",
    arguments: { symbol: "AAPL", provider: "fmp" },
  });
});

test("workbench tools persist user files but cannot mutate app-owned attachments", async (t) => {
  const temporary = mkdtempSync(join(tmpdir(), "guruterminal-workbench-"));
  const workbench = join(temporary, "workbench");
  const hostContextPath = join(temporary, "host-context.json");
  const previousCwd = process.cwd();
  const previousHostContext = process.env.GURUTERMINAL_HOST_CONTEXT_FILE;
  const previousSkillFiles = process.env.GURUTERMINAL_SKILL_FILES;
  writeFileSync(
    hostContextPath,
    JSON.stringify(runtimeContext("chat", ["read", "write", "edit", "ls", "find", "grep"])),
    { mode: 0o600 },
  );
  await import("node:fs/promises").then(({ mkdir }) => mkdir(workbench));
  await startToolBroker(t, workbenchBrokerHandler());
  process.chdir(workbench);
  process.env.GURUTERMINAL_HOST_CONTEXT_FILE = hostContextPath;
  process.env.GURUTERMINAL_SKILL_FILES = "[]";
  process.env.GURUTERMINAL_MODEL_RUN_OPTIONS = "{}";

  t.after(() => {
    process.chdir(previousCwd);
    if (previousHostContext === undefined) delete process.env.GURUTERMINAL_HOST_CONTEXT_FILE;
    else process.env.GURUTERMINAL_HOST_CONTEXT_FILE = previousHostContext;
    if (previousSkillFiles === undefined) delete process.env.GURUTERMINAL_SKILL_FILES;
    else process.env.GURUTERMINAL_SKILL_FILES = previousSkillFiles;
    rmSync(temporary, { recursive: true, force: true });
  });

  const tools = new Map();
  guruTerminalExtension({
    registerTool: (tool) => tools.set(tool.name, tool),
    on: () => {},
  });

  const written = await tools.get("write").execute("write", {
    path: "notes/idea.md",
    content: "durable insight",
  });
  assert.equal(written.details.status, "ok");
  assert.equal(written.details.path, "notes/idea.md");
  const result = await tools.get("read").execute("read", {
    path: "notes/idea.md",
  });
  assert.equal(result.details.content, "durable insight");
  assert.equal(result.details.revision.length, 64);
  assert.equal(result.details.result_ref, "result:workbench-read");
  await assert.rejects(
    () => tools.get("read").execute("escape", { path: "../outside.txt" }),
    /outside this Guru's workbench/,
  );

  const attachmentDirectory = join(workbench, "attachments", "chat-a", "message-a");
  const attachmentPath = join(attachmentDirectory, "attachment-a");
  mkdirSync(attachmentDirectory, { recursive: true, mode: 0o700 });
  writeFileSync(attachmentPath, "immutable attachment", { mode: 0o600 });
  const attachment = await tools.get("read").execute("read", {
    path: "attachments/chat-a/message-a/attachment-a",
  });
  assert.equal(attachment.details.content, "immutable attachment");
  await assert.rejects(
    () =>
      tools.get("write").execute("write-attachment", {
        path: "attachments/chat-a/message-a/attachment-a",
        content: "overwritten",
      }),
    /attachment snapshots are read-only/,
  );
  await assert.rejects(
    () =>
      tools.get("edit").execute("edit-attachment", {
        path: "attachments/chat-a/message-a/attachment-a",
        old_text: "immutable",
        new_text: "changed",
        expected_revision: "a".repeat(64),
      }),
    /attachment snapshots are read-only/,
  );
  await assert.rejects(
    () =>
      tools.get("write").execute("new-attachment", {
        path: "attachments/chat-a/message-a/injected",
        content: "injected",
      }),
    /attachment snapshots are read-only/,
  );
  assert.equal(readFileSync(attachmentPath, "utf8"), "immutable attachment");

  mkdirSync(join(workbench, "notes"), { recursive: true, mode: 0o700 });
  writeFileSync(join(workbench, "notes", "alpha.md"), "one\nmatch here\nthree\n", { mode: 0o600 });
  writeFileSync(join(workbench, "notes", "data.bin"), Buffer.from([0, 1, 2]), { mode: 0o600 });
  const grep = await tools.get("grep").execute("grep", { pattern: "match", context: 1 });
  assert.match(grep.content[0].text, /notes\/alpha\.md-1-one/);
  assert.match(grep.content[0].text, /notes\/alpha\.md:2:match here/);
  assert.equal(grep.details.skipped_binary, 1);
  assert.match(grep.content[0].text, /Skipped 1 binary file/);
});

test("read exposes only exact enabled Skill files and never grants write access", async (t) => {
  const temporary = mkdtempSync(join(tmpdir(), "guruterminal-skills-"));
  const workbench = join(temporary, "workbench");
  const selectedSkill = join(temporary, "selected-SKILL.md");
  const otherSkill = join(temporary, "other-SKILL.md");
  const hostContextPath = join(temporary, "host-context.json");
  const previousCwd = process.cwd();
  const previousHostContext = process.env.GURUTERMINAL_HOST_CONTEXT_FILE;
  const previousSkillFiles = process.env.GURUTERMINAL_SKILL_FILES;
  await import("node:fs/promises").then(({ mkdir }) => mkdir(workbench));
  writeFileSync(selectedSkill, "# Enabled workflow\n", { mode: 0o600 });
  writeFileSync(otherSkill, "# Disabled workflow\n", { mode: 0o600 });
  writeFileSync(
    hostContextPath,
    JSON.stringify(runtimeContext("chat", ["read", "write"])),
    { mode: 0o600 },
  );
  await startToolBroker(t, workbenchBrokerHandler());
  process.chdir(workbench);
  process.env.GURUTERMINAL_HOST_CONTEXT_FILE = hostContextPath;
  process.env.GURUTERMINAL_SKILL_FILES = JSON.stringify([selectedSkill]);
  process.env.GURUTERMINAL_MODEL_RUN_OPTIONS = "{}";

  t.after(() => {
    process.chdir(previousCwd);
    if (previousHostContext === undefined) delete process.env.GURUTERMINAL_HOST_CONTEXT_FILE;
    else process.env.GURUTERMINAL_HOST_CONTEXT_FILE = previousHostContext;
    if (previousSkillFiles === undefined) delete process.env.GURUTERMINAL_SKILL_FILES;
    else process.env.GURUTERMINAL_SKILL_FILES = previousSkillFiles;
    rmSync(temporary, { recursive: true, force: true });
  });

  const tools = new Map();
  guruTerminalExtension({
    registerTool: (tool) => tools.set(tool.name, tool),
    on: () => {},
  });
  assert.equal(process.env.GURUTERMINAL_SKILL_FILES, undefined);
  const result = await tools.get("read").execute("read", { path: selectedSkill });
  assert.equal(result.content[0].text, "# Enabled workflow\n");
  assert.equal(result.details.access, "enabled_skill");
  await assert.rejects(
    () => tools.get("read").execute("read", { path: otherSkill }),
    /outside this Guru's workbench/,
  );
  await assert.rejects(
    () => tools.get("write").execute("write", { path: selectedSkill, content: "changed" }),
    /outside this Guru's workbench/,
  );
});

test("always-on method Skills are readable with research and never writable", async (t) => {
  const agentDir = dirname(fileURLToPath(import.meta.url));
  const researchSkill = join(agentDir, "skills/research/SKILL.md");
  const valuationSkill = join(agentDir, "skills/valuation/SKILL.md");
  const temporary = mkdtempSync(join(tmpdir(), "guruterminal-always-on-"));
  const workbench = join(temporary, "workbench");
  const hostContextPath = join(temporary, "host-context.json");
  const previousCwd = process.cwd();
  const previousHostContext = process.env.GURUTERMINAL_HOST_CONTEXT_FILE;
  const previousSkillFiles = process.env.GURUTERMINAL_SKILL_FILES;
  await import("node:fs/promises").then(({ mkdir }) => mkdir(workbench));
  writeFileSync(
    hostContextPath,
    JSON.stringify(runtimeContext("chat", ["read", "write"])),
    { mode: 0o600 },
  );
  await startToolBroker(t, workbenchBrokerHandler());
  process.chdir(workbench);
  process.env.GURUTERMINAL_HOST_CONTEXT_FILE = hostContextPath;
  process.env.GURUTERMINAL_SKILL_FILES = JSON.stringify([
    valuationSkill,
    researchSkill,
  ]);
  process.env.GURUTERMINAL_MODEL_RUN_OPTIONS = "{}";

  t.after(() => {
    process.chdir(previousCwd);
    if (previousHostContext === undefined) delete process.env.GURUTERMINAL_HOST_CONTEXT_FILE;
    else process.env.GURUTERMINAL_HOST_CONTEXT_FILE = previousHostContext;
    if (previousSkillFiles === undefined) delete process.env.GURUTERMINAL_SKILL_FILES;
    else process.env.GURUTERMINAL_SKILL_FILES = previousSkillFiles;
    rmSync(temporary, { recursive: true, force: true });
  });

  const tools = new Map();
  guruTerminalExtension({
    registerTool: (tool) => tools.set(tool.name, tool),
    on: () => {},
  });
  const research = await tools.get("read").execute("read", { path: researchSkill });
  const valuation = await tools.get("read").execute("read", { path: valuationSkill });
  assert.equal(research.details.access, "enabled_skill");
  assert.equal(valuation.details.access, "enabled_skill");
  assert.notEqual(research.content[0].text, valuation.content[0].text);
  assert.ok(research.content[0].text.length > 0);
  assert.ok(valuation.content[0].text.length > 0);
  await assert.rejects(
    () =>
      tools.get("write").execute("write", {
        path: valuationSkill,
        content: "changed",
      }),
    /outside this Guru's workbench/,
  );
});
