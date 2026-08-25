import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";
import { getBuiltinModels } from "@earendil-works/pi-ai/providers/all";
import {
  buildAnthropicBody,
  buildCodexBody,
  buildXaiBody,
  classifyHttpFailure,
  completedAnthropicTransportPrefix,
  completedCodexTransportPrefix,
  isCodexResponsesLiteModel,
  parseAnthropicSearchEvents,
  parseAnthropicSearchPayload,
  parseAnthropicSearchResponse,
  parseCodexJsonTransport,
  parseCodexSearchEvents,
  parseCodexSearchPayload,
  parseSseJsonEvents,
  parseXaiSearchResponse,
  runAnthropicSearchThroughPi,
  runNativeSearch,
  selectSearchModel,
  toSearchErrorResult,
  toSearchResult,
} from "./guruterminal-native-search.mjs";

test("pinned Pi catalog includes Grok 4.6 for xAI", () => {
  const model = getBuiltinModels("xai").find((candidate) => candidate.id === "grok-4.6");
  assert.equal(model?.name, "Grok 4.6");
  assert.deepEqual(model?.input, ["text", "image"]);
  assert.equal(model?.contextWindow, 500_000);
  assert.equal(model?.maxTokens, 500_000);
});

test("provider result writes retry until every byte is published", async () => {
  const moduleUrl = `${pathToFileURL(join(import.meta.dirname, "guruterminal-provider-extension.mjs"))}?write=${Date.now()}`;
  const { writeExactSync } = await import(moduleUrl);
  const encoded = Buffer.from("bounded-result", "utf8");
  const published = [];
  writeExactSync(7, encoded, (descriptor, buffer, offset, length, position) => {
    assert.equal(descriptor, 7);
    assert.equal(position, null);
    const written = Math.min(2, length);
    published.push(buffer.subarray(offset, offset + written));
    return written;
  });
  assert.equal(Buffer.concat(published).toString("utf8"), "bounded-result");
  assert.throws(
    () => writeExactSync(7, encoded, () => 0),
    /result write made no progress/,
  );
});

test("xAI subscription OAuth skips native search without exposing the credential", async () => {
  const moduleUrl = `${pathToFileURL(join(import.meta.dirname, "guruterminal-provider-extension.mjs"))}?xai-oauth=${Date.now()}`;
  const { resolveSearchAuth } = await import(moduleUrl);
  await assert.rejects(
    () =>
      resolveSearchAuth(
        {
          modelRegistry: {
            getProvider: () => ({ id: "xai" }),
            getProviderAuth: async () => ({
              source: "OAuth",
              auth: { apiKey: "xai-oauth-redacted-test" },
            }),
          },
        },
        "xai",
      ),
    (error) => {
      assert.equal(error.kind, "unavailable");
      assert.equal(error.message.includes("xai-oauth-redacted-test"), false);
      return true;
    },
  );
});

test("persists credentials through Pi storage without returning secrets", async (t) => {
  const temporary = mkdtempSync(join(tmpdir(), "guruterminal-provider-extension-"));
  const resultPath = join(temporary, "result.json");
  writeFileSync(resultPath, "stale result".repeat(8_192), { mode: 0o600 });
  process.env.PI_CODING_AGENT_DIR = temporary;
  process.env.GURUTERMINAL_PROVIDER_RESULT_FILE = resultPath;
  process.env.GURUTERMINAL_PROVIDER_API_KEY = "api-key-secret";
  t.after(() => {
    delete process.env.PI_CODING_AGENT_DIR;
    delete process.env.GURUTERMINAL_PROVIDER_RESULT_FILE;
    delete process.env.GURUTERMINAL_PROVIDER_API_KEY;
    rmSync(temporary, { recursive: true, force: true });
  });

  const moduleUrl = `${pathToFileURL(join(import.meta.dirname, "guruterminal-provider-extension.mjs"))}?test=${Date.now()}`;
  const { default: extension } = await import(moduleUrl);
  const commands = new Map();
  extension({
    registerCommand: (name, command) => commands.set(name, command),
  });
  assert.deepEqual([...commands.keys()].sort(), [
    "guruterminal-provider-api-key",
    "guruterminal-provider-login",
    "guruterminal-provider-logout",
    "guruterminal-provider-models",
    "guruterminal-provider-search",
  ]);
  assert.equal(process.env.GURUTERMINAL_PROVIDER_RESULT_FILE, undefined);
  assert.equal(process.env.GURUTERMINAL_PROVIDER_REQUEST_FILE, undefined);
  assert.equal(process.env.GURUTERMINAL_PROVIDER_API_KEY, undefined);

  const notices = [];
  const models = [
    {
      provider: "openai-codex",
      id: "gpt-test",
      name: "GPT Test",
      reasoning: true,
      contextWindow: 128_000,
      maxTokens: 32_000,
      input: ["text", "image"],
      thinkingLevelMap: { off: "off", xhigh: "xhigh", max: null },
      api: "openai-codex-responses",
    },
    {
      provider: "openai-codex",
      id: "gpt-unavailable",
      name: "Unavailable GPT",
      reasoning: true,
      contextWindow: 128_000,
      maxTokens: 32_000,
    },
    {
      provider: "openai-codex",
      id: "gpt-always-thinking",
      name: "Always Thinking GPT",
      reasoning: true,
      contextWindow: 128_000,
      maxTokens: 32_000,
      input: ["text"],
      thinkingLevelMap: {
        off: null,
        minimal: null,
        low: null,
        medium: null,
        high: "high",
        xhigh: null,
        max: "max",
      },
      api: "openai-codex-responses",
    },
  ];
  const oauthProvider = {
    id: "openai-codex",
    name: "OpenAI with ChatGPT",
    auth: {
      oauth: {
        login: async (interaction) => {
          assert.equal(
            await interaction.prompt({
              type: "select",
              options: [
                { id: "browser", label: "Browser" },
                { id: "device_code", label: "Device code" },
              ],
            }),
            "browser",
          );
          const manualPrompt = new AbortController();
          interaction.notify({
            type: "auth_url",
            url: "https://auth.openai.com/oauth/authorize?client_id=test",
            instructions: "Complete login in your browser.",
          });
          const callbackWait = interaction.prompt({
            type: "manual_code",
            signal: manualPrompt.signal,
          });
          manualPrompt.abort();
          await assert.rejects(callbackWait, { name: "AbortError" });
          return {
            type: "oauth",
            access: "access-secret",
            refresh: "refresh-secret",
            expires: 1_900_000_000_000,
          };
        },
        refresh: async (credential) => credential,
        toAuth: async (credential) => ({ auth: { apiKey: credential.access } }),
      },
    },
    getModels: () => models,
    stream: () => {
      throw new Error("unused in provider setup test");
    },
    streamSimple: () => {
      throw new Error("unused in provider setup test");
    },
  };
  const apiKeyProvider = {
    id: "anthropic",
    name: "Anthropic",
    auth: {
      apiKey: {
        name: "API key",
        login: async (interaction) => ({
          type: "api_key",
          key: await interaction.prompt({ type: "secret", message: "API key" }),
        }),
        resolve: async ({ credential }) =>
          credential ? { auth: { apiKey: credential.key } } : undefined,
      },
    },
    getModels: () => [],
    stream: () => {
      throw new Error("unused in provider setup test");
    },
    streamSimple: () => {
      throw new Error("unused in provider setup test");
    },
  };
  const context = {
    modelRegistry: {
      getAll: () => models,
      getAvailable: () => [models[0], models[2]],
      getProvider: (providerId) =>
        providerId === "openai-codex" ? oauthProvider : apiKeyProvider,
    },
    ui: {
      notify: (message) => notices.push(message),
    },
  };

  await commands.get("guruterminal-provider-login").handler("openai-codex", context);
  const result = JSON.parse(readFileSync(resultPath, "utf8"));
  assert.equal(result.protocol, "guruterminal-provider/1");
  assert.equal(result.type, "credential_updated");
  assert.equal("credential" in result, false);
  assert.deepEqual(result.models, []);

  await commands
    .get("guruterminal-provider-models")
    .handler("openai-codex", context);
  const modelResult = JSON.parse(readFileSync(resultPath, "utf8"));
  const standardModel = modelResult.models.find((model) => model.id === "gpt-test");
  assert.deepEqual(standardModel.thinking_levels, ["off", "minimal", "low", "medium", "high", "xhigh"]);
  assert.equal(standardModel.thinking_level_map.max, null);
  assert.deepEqual(standardModel.run_controls, [
    {
      id: "performance",
      label: "Performance",
      default_choice: "standard",
      choices: [
        {
          id: "standard",
          label: "Standard",
          description: "Use the provider's standard service tier.",
        },
        {
          id: "fast",
          label: "Fast",
          description: "Request the provider's priority service tier.",
        },
      ],
    },
  ]);
  assert.deepEqual(
    modelResult.models.find((model) => model.id === "gpt-always-thinking").thinking_levels,
    ["high", "max"],
  );
  assert.equal(modelResult.models.some((model) => model.id === "gpt-unavailable"), false);

  await commands
    .get("guruterminal-provider-api-key")
    .handler("anthropic set", context);
  const saved = JSON.parse(readFileSync(join(temporary, "auth.json"), "utf8"));
  assert.equal(saved["openai-codex"].access, "access-secret");
  assert.equal(saved.anthropic.key, "api-key-secret");
  const mutationResult = JSON.parse(readFileSync(resultPath, "utf8"));
  assert.equal(mutationResult.type, "credential_updated");
  assert.equal("credential" in mutationResult, false);

  await commands
    .get("guruterminal-provider-api-key")
    .handler("anthropic clear", context);
  const cleared = JSON.parse(readFileSync(join(temporary, "auth.json"), "utf8"));
  assert.equal(cleared.anthropic, undefined);
  assert.equal(cleared["openai-codex"].refresh, "refresh-secret");

  await commands.get("guruterminal-provider-logout").handler("openai-codex", context);
  const loggedOut = JSON.parse(readFileSync(join(temporary, "auth.json"), "utf8"));
  assert.equal(loggedOut["openai-codex"], undefined);
  const logoutResult = JSON.parse(readFileSync(resultPath, "utf8"));
  assert.equal(logoutResult.type, "credential_updated");
  assert.equal(logoutResult.provider, "openai-codex");
  assert.equal("credential" in logoutResult, false);
  assert.equal(
    notices.some((notice) => notice.includes("auth.openai.com/oauth/authorize")),
    true,
  );
  assert.equal(notices.some((notice) => notice.includes("device_code")), false);
  assert.equal(notices.some((notice) => notice.includes("access-secret")), false);
  assert.equal(notices.some((notice) => notice.includes("refresh-secret")), false);
});

test("forwards a device verification URL without exposing the user code", async (t) => {
  const temporary = mkdtempSync(join(tmpdir(), "guruterminal-provider-device-"));
  const resultPath = join(temporary, "result.json");
  writeFileSync(resultPath, "", { mode: 0o600 });
  process.env.PI_CODING_AGENT_DIR = temporary;
  process.env.GURUTERMINAL_PROVIDER_RESULT_FILE = resultPath;
  t.after(() => {
    delete process.env.PI_CODING_AGENT_DIR;
    delete process.env.GURUTERMINAL_PROVIDER_RESULT_FILE;
    rmSync(temporary, { recursive: true, force: true });
  });

  const moduleUrl = `${pathToFileURL(join(import.meta.dirname, "guruterminal-provider-extension.mjs"))}?device=${Date.now()}`;
  const { default: extension } = await import(moduleUrl);
  const commands = new Map();
  extension({
    registerCommand: (name, command) => commands.set(name, command),
  });

  const notices = [];
  const deviceProvider = {
    id: "xai",
    name: "xAI",
    auth: {
      oauth: {
        login: async (interaction) => {
          interaction.notify({
            type: "device_code",
            userCode: "WXYZ-1234",
            verificationUri: "https://accounts.x.ai/oauth2/device?user_code=WXYZ-1234",
          });
          return {
            type: "oauth",
            access: "xai-access",
            refresh: "xai-refresh",
            expires: 1_900_000_000_000,
          };
        },
        refresh: async (credential) => credential,
        toAuth: async (credential) => ({ auth: { apiKey: credential.access } }),
      },
    },
    getModels: () => [],
    stream: () => {
      throw new Error("unused in provider setup test");
    },
    streamSimple: () => {
      throw new Error("unused in provider setup test");
    },
  };
  const context = {
    modelRegistry: {
      getProvider: () => deviceProvider,
    },
    ui: {
      notify: (message) => notices.push(message),
    },
  };

  await commands.get("guruterminal-provider-login").handler("xai", context);
  const result = JSON.parse(readFileSync(resultPath, "utf8"));
  assert.equal(result.type, "credential_updated");
  assert.equal(result.provider, "xai");
  assert.equal("credential" in result, false);
  assert.equal(
    notices.some((notice) =>
      notice.includes("https://accounts.x.ai/oauth2/device?user_code=WXYZ-1234"),
    ),
    true,
  );
  assert.equal(notices.some((notice) => notice.includes("\"type\":\"authorization_url\"")), true);
  assert.equal(notices.some((notice) => notice.includes("userCode")), false);
  assert.equal(notices.some((notice) => notice.includes("xai-access")), false);
});

const CODEX_SEARCH_SSE = [
  `data: ${JSON.stringify({ type: "response.web_search_call.completed", item_id: "ws_test" })}`,
  "",
  `data: ${JSON.stringify({
    type: "response.output_item.done",
    item: {
      type: "web_search_call",
      action: { sources: [{ url: "https://example.com/article?utm_source=openai", title: "Example Article" }] },
    },
  })}`,
  "",
  `data: ${JSON.stringify({
    type: "response.output_item.done",
    item: {
      type: "message",
      content: [
        {
          type: "output_text",
          text: "Provider synthesized answer that must be discarded.",
          annotations: [
            { type: "url_citation", url: "https://example.com/article", title: "Example Article", start_index: 0, end_index: 8 },
          ],
        },
      ],
    },
  })}`,
  "",
  `data: ${JSON.stringify({
    type: "response.completed",
    response: {
      id: "resp_codex_test",
      model: "gpt-5.6-luna",
      usage: { input_tokens: 12, output_tokens: 7, total_tokens: 19 },
    },
  })}`,
  "",
].join("\n");

const ANTHROPIC_SEARCH_JSON = {
  id: "msg_test",
  model: "claude-haiku-4-5",
  content: [
    { type: "server_tool_use", name: "web_search", input: { query: "latest climate report" } },
    {
      type: "web_search_tool_result",
      content: [
        {
          type: "web_search_result",
          title: "UN report",
          url: "https://example.org/un-report",
          page_age: "2 days ago",
        },
      ],
    },
    {
      type: "text",
      text: "Anthropic synthesized answer",
      citations: [{ url: "https://example.org/un-report", title: "UN report", cited_text: "emissions fell" }],
    },
  ],
  usage: {
    input_tokens: 20,
    output_tokens: 8,
    server_tool_use: { web_search_requests: 1 },
  },
};

const ANTHROPIC_SEARCH_SSE = [
  "event: message_start",
  `data: ${JSON.stringify({
    type: "message_start",
    message: {
      id: "msg_stream_test",
      model: "claude-haiku-4-5",
      usage: { input_tokens: 21, output_tokens: 0 },
    },
  })}`,
  "",
  "event: content_block_start",
  `data: ${JSON.stringify({
    type: "content_block_start",
    index: 0,
    content_block: { type: "server_tool_use", id: "srvtoolu_test", name: "web_search" },
  })}`,
  "",
  "event: content_block_start",
  `data: ${JSON.stringify({
    type: "content_block_start",
    index: 1,
    content_block: {
      type: "web_search_tool_result",
      tool_use_id: "srvtoolu_test",
      content: [
        {
          type: "web_search_result",
          title: "Streaming report",
          url: "https://example.org/streaming-report",
          page_age: "3 hours ago",
        },
      ],
    },
  })}`,
  "",
  "event: content_block_start",
  `data: ${JSON.stringify({
    type: "content_block_start",
    index: 2,
    content_block: { type: "text", text: "" },
  })}`,
  "",
  "event: content_block_delta",
  `data: ${JSON.stringify({
    type: "content_block_delta",
    index: 2,
    delta: {
      type: "citations_delta",
      citation: {
        type: "web_search_result_location",
        url: "https://example.org/streaming-report",
        title: "Streaming report",
        cited_text: "Cited source excerpt",
      },
    },
  })}`,
  "",
  "event: message_delta",
  `data: ${JSON.stringify({
    type: "message_delta",
    usage: { output_tokens: 9, server_tool_use: { web_search_requests: 1 } },
  })}`,
  "",
  "event: message_stop",
  `data: ${JSON.stringify({ type: "message_stop" })}`,
  "",
].join("\n");

test("parses Codex hosted search sources and usage without keeping the answer", () => {
  const parsed = parseCodexSearchEvents(parseSseJsonEvents(CODEX_SEARCH_SSE));
  const result = toSearchResult("openai-codex", parsed, 5);
  assert.equal(result.status, "ok");
  assert.equal(result.sources[0].url, "https://example.com/article");
  assert.equal(result.sources[0].title, "Example Article");
  assert.equal(result.model, "gpt-5.6-luna");
  assert.equal(result.requestId, "resp_codex_test");
  assert.equal(result.usage.inputTokens, 12);
  assert.equal("answer" in result, false);
  assert.equal(JSON.stringify(result).includes("Provider synthesized answer"), false);
});

test("keeps a Codex hosted search with no sources empty instead of mining the answer", () => {
  const sse = [
    `data: ${JSON.stringify({ type: "response.web_search_call.completed", item_id: "ws_empty" })}`,
    "",
    `data: ${JSON.stringify({
      type: "response.output_item.done",
      item: { type: "web_search_call", action: { sources: [] } },
    })}`,
    "",
    `data: ${JSON.stringify({
      type: "response.output_item.done",
      item: {
        type: "message",
        content: [
          {
            type: "output_text",
            text: "See https://invented.example/from-answer and [Report](https://invented.example/md).",
          },
        ],
      },
    })}`,
    "",
    `data: ${JSON.stringify({
      type: "response.completed",
      response: { id: "resp_empty_search", model: "gpt-5.6-luna" },
    })}`,
    "",
  ].join("\n");
  const parsed = parseCodexSearchEvents(parseSseJsonEvents(sse));
  const result = toSearchResult("openai-codex", parsed, 5);
  assert.equal(result.status, "ok");
  assert.deepEqual(result.sources, []);
});

test("fails Codex completions that never invoke hosted web search", () => {
  const sse = [
    `data: ${JSON.stringify({
      type: "response.output_item.done",
      item: { type: "message", content: [{ type: "output_text", text: "stale answer, no search" }] },
    })}`,
    "",
    `data: ${JSON.stringify({ type: "response.completed", response: { id: "resp_no_search", model: "gpt-5.6-luna" } })}`,
    "",
  ].join("\n");
  assert.throws(
    () => parseCodexSearchEvents(parseSseJsonEvents(sse)),
    /without running web search/,
  );
});

test("prefers classic GPT-5.5 for nested search while recognizing Luna as Responses-Lite", () => {
  assert.equal(
    selectSearchModel("openai-codex", ["gpt-5.6-luna", "gpt-5.5", "gpt-5.4-mini"]),
    "gpt-5.5",
  );
  assert.equal(isCodexResponsesLiteModel("gpt-5.6-luna"), true);
  assert.equal(isCodexResponsesLiteModel("gpt-5.5"), false);
});

test("classifies Codex tool_choice mismatch as a provider error", () => {
  assert.equal(
    classifyHttpFailure(
      400,
      "Tool choice 'web_search_preview' not found in 'tools' parameter.",
    ),
    "provider",
  );
  assert.equal(classifyHttpFailure(402, "payment required"), "unavailable");
  assert.equal(classifyHttpFailure(400, "insufficient_quota"), "unavailable");
});

test("keeps the dedicated Codex search on the classic forced web_search contract", () => {
  const luna = buildCodexBody({ query: "latest public report", limit: 5 }, "gpt-5.6-luna");
  assert.equal(luna.model, "gpt-5.6-luna");
  assert.deepEqual(luna.tool_choice, { type: "web_search" });
  assert.deepEqual(luna.tools, [{ type: "web_search", search_context_size: "high" }]);
  assert.equal(luna.reasoning.effort, "low");
  assert.deepEqual(luna.include, ["web_search_call.action.sources"]);
  assert.equal(Array.isArray(luna.input) && luna.input[0]?.type === "additional_tools", false);
  assert.equal("instructions" in luna, true);
  assert.equal(JSON.stringify(luna).includes("web_search_preview"), false);

  const classic = buildCodexBody({ query: "latest public report", limit: 5 }, "gpt-5.5");
  assert.deepEqual(classic.tool_choice, { type: "web_search" });
  assert.deepEqual(classic.tools, [{ type: "web_search", search_context_size: "high" }]);
  assert.equal(classic.reasoning.effort, "low");
});

test("parses Codex sources from response.completed output when incremental search events are omitted", () => {
  const sse = [
    `data: ${JSON.stringify({
      type: "response.completed",
      response: {
        id: "resp_luna_completed",
        model: "gpt-5.6-luna",
        usage: { input_tokens: 40, output_tokens: 12, total_tokens: 52 },
        output: [
          {
            type: "web_search_call",
            action: {
              sources: [{ url: "https://example.com/report", title: "Public Report" }],
            },
          },
          {
            type: "message",
            content: [
              {
                type: "output_text",
                text: "Provider synthesized answer that must be discarded.",
                annotations: [
                  {
                    type: "url_citation",
                    url: "https://example.com/report",
                    title: "Public Report",
                    cited_text: "reported figure",
                  },
                ],
              },
            ],
          },
        ],
      },
    })}`,
    "",
  ].join("\n");
  const parsed = parseCodexSearchEvents(parseSseJsonEvents(sse));
  const result = toSearchResult("openai-codex", parsed, 5);
  assert.equal(result.status, "ok");
  assert.equal(result.model, "gpt-5.6-luna");
  assert.equal(result.requestId, "resp_luna_completed");
  assert.equal(result.sources[0].url, "https://example.com/report");
  assert.equal(result.sources[0].title, "Public Report");
  assert.equal(result.sources[0].snippet, "reported figure");
  assert.equal(JSON.stringify(result).includes("Provider synthesized answer"), false);
});

test("citations on a completed Codex message still do not count as hosted web search", () => {
  const sse = [
    `data: ${JSON.stringify({
      type: "response.completed",
      response: {
        id: "resp_cited_only",
        model: "gpt-5.6-luna",
        output: [
          {
            type: "message",
            content: [
              {
                type: "output_text",
                text: "stale",
                annotations: [
                  { type: "url_citation", url: "https://example.com/cited-only", title: "Cited" },
                ],
              },
            ],
          },
        ],
      },
    })}`,
    "",
  ].join("\n");
  assert.throws(
    () => parseCodexSearchEvents(parseSseJsonEvents(sse)),
    /without running web search/,
  );
});

test("parses Anthropic hosted search sources and usage without keeping the answer", () => {
  const parsed = parseAnthropicSearchResponse(ANTHROPIC_SEARCH_JSON);
  const result = toSearchResult("anthropic", parsed, 5);
  assert.equal(result.sources[0].url, "https://example.org/un-report");
  assert.equal(result.usage.searchRequests, 1);
  assert.equal(result.searchRequestCount, 1);
  assert.equal("answer" in result, false);
});

test("parses Anthropic streaming server-tool blocks and stops at message_stop", () => {
  const prefix = completedAnthropicTransportPrefix(`${ANTHROPIC_SEARCH_SSE}never-closes`);
  assert.equal(prefix, ANTHROPIC_SEARCH_SSE);
  const parsed = parseAnthropicSearchPayload(ANTHROPIC_SEARCH_SSE);
  const fromEvents = parseAnthropicSearchEvents(parseSseJsonEvents(ANTHROPIC_SEARCH_SSE));
  for (const value of [parsed, fromEvents]) {
    const result = toSearchResult("anthropic", value, 5);
    assert.equal(result.requestId, "msg_stream_test");
    assert.equal(result.model, "claude-haiku-4-5");
    assert.equal(result.sources[0].url, "https://example.org/streaming-report");
    assert.equal(result.sources[0].snippet, "Cited source excerpt");
    assert.equal(result.usage.inputTokens, 21);
    assert.equal(result.usage.outputTokens, 9);
    assert.equal(result.usage.searchRequests, 1);
    assert.equal("answer" in result, false);
  }
});

test("keeps Pi Anthropic OAuth system blocks while replacing only the search payload", async () => {
  let capturedBody;
  const ctx = {
    modelRegistry: {
      find: (provider, model) =>
        provider === "anthropic" && model === "claude-haiku-4-5"
          ? { provider, id: model }
          : undefined,
      complete: async (_model, _context, options) => {
        capturedBody = await options.onPayload({
          system: [
            { type: "text", text: "Claude Code system fingerprint", cache_control: { type: "ephemeral" } },
          ],
          messages: [{ role: "user", content: "placeholder" }],
        });
        const response = await options.fetch("https://api.anthropic.com/v1/messages", {
          method: "POST",
          body: JSON.stringify({ ...capturedBody, stream: true }),
        });
        await response.text();
        return { role: "assistant", content: [] };
      },
    },
  };
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async () =>
    new Response(ANTHROPIC_SEARCH_SSE, {
      status: 200,
      headers: { "Content-Type": "text/event-stream" },
    });
  try {
    const payload = await runAnthropicSearchThroughPi(ctx, {
      model: "claude-haiku-4-5",
      request: {
        query: "latest primary report",
        limit: 3,
        include_domains: ["example.org"],
      },
      signal: new AbortController().signal,
    });
    const result = toSearchResult("anthropic", parseAnthropicSearchPayload(await payload), 3);
    assert.equal(result.sources[0].url, "https://example.org/streaming-report");
  } finally {
    globalThis.fetch = originalFetch;
  }
  assert.deepEqual(capturedBody.system, [
    { type: "text", text: "Claude Code system fingerprint", cache_control: { type: "ephemeral" } },
  ]);
  assert.equal(capturedBody.max_tokens, 4096);
  assert.deepEqual(capturedBody.tools, [
    {
      type: "web_search_20250305",
      name: "web_search",
      allowed_domains: ["example.org"],
    },
  ]);
});

test("fails Anthropic 200 responses that contain a tool error", () => {
  assert.throws(
    () =>
      parseAnthropicSearchResponse({
        id: "msg_err",
        model: "claude-haiku-4-5",
        content: [
          { type: "server_tool_use", name: "web_search", input: { query: "q" } },
          {
            type: "web_search_tool_result",
            is_error: true,
            content: { type: "web_search_tool_result_error", error_code: "unavailable" },
          },
        ],
        usage: { input_tokens: 1, output_tokens: 1 },
      }),
    (error) => error?.kind === "unavailable" && /unavailable/.test(error.message),
  );
});

test("parses xAI annotation and web_search_call source variants", () => {
  const parsed = parseXaiSearchResponse({
    id: "resp_xai_123",
    model: "grok-4.6",
    output_text: "Top-level xAI answer",
    annotations: [
      { type: "url_citation", url: "https://example.com/top-annotation", title: "Top Annotation", text: "Top annotation text" },
    ],
    output: [
      {
        type: "message",
        content: [
          {
            type: "output_text",
            text: "The cited sentence appears inside this provider answer and must be discarded.",
            annotations: [
              {
                type: "url_citation",
                url: "https://example.com/cited",
                title: "Cited",
                text: "Cited source excerpt",
              },
            ],
          },
        ],
      },
      {
        type: "web_search_call",
        action: { sources: [{ url: "https://example.com/raw", title: "Raw result" }] },
      },
    ],
    citations: ["https://example.com/top-level-citation"],
    usage: { input_tokens: 12, output_tokens: 8, total_tokens: 20 },
  });
  const result = toSearchResult("xai", parsed, 10);
  assert.deepEqual(
    result.sources.map((source) => source.url),
    [
      "https://example.com/top-annotation",
      "https://example.com/cited",
      "https://example.com/raw",
      "https://example.com/top-level-citation",
    ],
  );
  assert.equal(result.usage.totalTokens, 20);
  assert.equal(result.sources[1].snippet, "Cited source excerpt");
  assert.equal("answer" in result, false);
  assert.equal(JSON.stringify(result).includes("Top-level xAI answer"), false);
});

test("uses xAI's dedicated Responses search model, low reasoning, and bounded domain filters", () => {
  assert.equal(selectSearchModel("xai", ["grok-4.6", "grok-4.5"]), "grok-4.5");
  const body = buildXaiBody(
    {
      query: "latest primary report",
      limit: 10,
      include_domains: ["a.example", "b.example", "c.example", "d.example", "e.example", "f.example"],
      exclude_domains: ["ignored.example"],
    },
    "grok-4.5",
  );
  assert.equal(body.model, "grok-4.5");
  assert.deepEqual(body.reasoning, { effort: "low" });
  assert.deepEqual(body.include, ["web_search_call.action.sources"]);
  assert.deepEqual(body.tools, [
    {
      type: "web_search",
      filters: {
        allowed_domains: ["a.example", "b.example", "c.example", "d.example", "e.example"],
      },
    },
  ]);
});

test("fails xAI completions that never invoke hosted web search", () => {
  assert.throws(
    () =>
      parseXaiSearchResponse({
        id: "resp_xai_no_tool",
        model: "grok-4.6",
        annotations: [
          { type: "url_citation", url: "https://example.com/cited-only", title: "Cited" },
        ],
        citations: ["https://example.com/cited-only"],
      }),
    /without running web search/,
  );
});

test("rejects invalid native-search input before resolving credentials", async () => {
  let resolvedAuth = false;
  await assert.rejects(
    () =>
      runNativeSearch(
        { provider: "xai", query: "q".repeat(4_097), limit: 5 },
        {
          resolveAuth: async () => {
            resolvedAuth = true;
            return { apiKey: "unused" };
          },
        },
      ),
    (error) => error?.kind === "malformed" && error?.message === "search query is invalid",
  );
  assert.equal(resolvedAuth, false);
  await assert.rejects(
    () =>
      runNativeSearch(
        {
          provider: "xai",
          query: "latest report",
          limit: 5,
          include_domains: ["not a hostname!"],
        },
        {
          resolveAuth: async () => {
            resolvedAuth = true;
            return { apiKey: "unused" };
          },
        },
      ),
    (error) => error?.kind === "malformed" && error?.message === "search domains are invalid",
  );
  assert.equal(resolvedAuth, false);
});

test("runs a mocked native search and omits credentials, headers, and answers from the result", async (t) => {
  const temporary = mkdtempSync(join(tmpdir(), "guruterminal-provider-search-"));
  const resultPath = join(temporary, "result.json");
  const requestPath = join(temporary, "request.json");
  writeFileSync(resultPath, "", { mode: 0o600 });
  writeFileSync(
    requestPath,
    JSON.stringify({
      protocol: "guruterminal-provider/1",
      type: "search",
      provider: "anthropic",
      query: "latest public report",
      limit: 3,
    }),
    { mode: 0o600 },
  );
  process.env.PI_CODING_AGENT_DIR = temporary;
  process.env.GURUTERMINAL_PROVIDER_RESULT_FILE = resultPath;
  process.env.GURUTERMINAL_PROVIDER_REQUEST_FILE = requestPath;
  t.after(() => {
    delete process.env.PI_CODING_AGENT_DIR;
    delete process.env.GURUTERMINAL_PROVIDER_RESULT_FILE;
    delete process.env.GURUTERMINAL_PROVIDER_REQUEST_FILE;
    rmSync(temporary, { recursive: true, force: true });
  });

  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (url, init) => {
    const body = JSON.parse(init.body);
    assert.equal(new URL(url).hostname, "api.anthropic.com");
    assert.equal(init.headers["x-api-key"], "sk-ant-secret");
    assert.equal(body.tools[0].name, "web_search");
    assert.equal("answer" in body, false);
    return new Response(JSON.stringify(ANTHROPIC_SEARCH_JSON), {
      status: 200,
      headers: { "Content-Type": "application/json" },
    });
  };
  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  const moduleUrl = `${pathToFileURL(join(import.meta.dirname, "guruterminal-provider-extension.mjs"))}?search=${Date.now()}`;
  const { default: extension } = await import(moduleUrl);
  const commands = new Map();
  extension({ registerCommand: (name, command) => commands.set(name, command) });
  await commands.get("guruterminal-provider-search").handler("", {
    modelRegistry: {
      getProvider: () => ({
        id: "anthropic",
        name: "Anthropic",
        auth: { apiKey: true },
        getModels: () => [],
        stream: () => {
          throw new Error("unused");
        },
        streamSimple: () => {
          throw new Error("unused");
        },
      }),
      getAvailable: () => [{ provider: "anthropic", id: "claude-haiku-4-5" }],
      getProviderAuth: async () => ({ auth: { apiKey: "sk-ant-secret" } }),
    },
  });
  const result = JSON.parse(readFileSync(resultPath, "utf8"));
  assert.equal(result.protocol, "guruterminal-provider/1");
  assert.equal(result.type, "search");
  assert.equal(result.status, "ok");
  assert.equal(result.provider, "anthropic");
  assert.equal(result.sources[0].url, "https://example.org/un-report");
  assert.equal("answer" in result, false);
  assert.equal("credential" in result, false);
  assert.equal(JSON.stringify(result).includes("sk-ant-secret"), false);
  assert.equal(JSON.stringify(result).includes("Anthropic synthesized answer"), false);
  assert.equal(JSON.stringify(result).includes("authorization"), false);
});

test("uses Bearer rather than x-api-key for Anthropic OAuth tokens", async () => {
  let capturedHeaders;
  const result = await runNativeSearch(
    { provider: "anthropic", query: "latest public report", limit: 3 },
    {
      availableModelIds: ["claude-haiku-4-5"],
      resolveAuth: async () => ({ apiKey: "sk-ant-oat-redacted-test" }),
      fetchImpl: async (_url, init) => {
        capturedHeaders = init.headers;
        return new Response(JSON.stringify(ANTHROPIC_SEARCH_JSON), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      },
    },
  );
  assert.equal(capturedHeaders["x-api-key"], undefined);
  assert.equal(capturedHeaders.authorization, "Bearer sk-ant-oat-redacted-test");
  assert.match(capturedHeaders["anthropic-beta"], /oauth-2025-04-20/);
  assert.equal(result.status, "ok");
  assert.equal(JSON.stringify(result).includes("sk-ant-oat-redacted-test"), false);
});

test("runs xAI hosted web search through the Responses API contract", async () => {
  let captured;
  const result = await runNativeSearch(
    {
      provider: "xai",
      query: "latest xAI API release",
      limit: 3,
      include_domains: ["docs.x.ai"],
    },
    {
      availableModelIds: ["grok-4.6", "grok-4.5"],
      resolveAuth: async () => ({ apiKey: "xai-redacted-test" }),
      fetchImpl: async (url, init) => {
        captured = { url: String(url), headers: init.headers, body: JSON.parse(init.body) };
        return new Response(
          JSON.stringify({
            id: "resp_xai_mock",
            model: "grok-4.5",
            output: [
              {
                type: "web_search_call",
                action: {
                  sources: [{ url: "https://docs.x.ai/developers/tools/web-search", title: "Web Search" }],
                },
              },
            ],
            usage: { input_tokens: 4, output_tokens: 2, total_tokens: 6 },
          }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        );
      },
    },
  );
  assert.match(captured.url, /api\.x\.ai\/v1\/responses$/);
  assert.equal(captured.body.model, "grok-4.5");
  assert.deepEqual(captured.body.reasoning, { effort: "low" });
  assert.deepEqual(captured.body.include, ["web_search_call.action.sources"]);
  assert.deepEqual(captured.body.tools, [
    { type: "web_search", filters: { allowed_domains: ["docs.x.ai"] } },
  ]);
  assert.equal(captured.headers.authorization, "Bearer xai-redacted-test");
  assert.equal(result.status, "ok");
  assert.equal(result.sources[0].url, "https://docs.x.ai/developers/tools/web-search");
  assert.equal(JSON.stringify(result).includes("xai-redacted-test"), false);
});

test("hard-times out stalled provider requests", async () => {
  const keepAlive = setTimeout(() => undefined, 100);
  try {
    await assert.rejects(
      () =>
        runNativeSearch(
          { provider: "xai", query: "bounded request", limit: 3 },
          {
            timeoutMs: 10,
            availableModelIds: ["grok-4.5"],
            resolveAuth: async () => ({ apiKey: "xai-redacted-timeout" }),
            fetchImpl: async (_url, init) =>
              new Promise((_resolve, reject) => {
                init.signal.addEventListener("abort", () => reject(init.signal.reason), { once: true });
              }),
          },
        ),
      (error) => error instanceof Error && error.kind === "timeout",
    );
  } finally {
    clearTimeout(keepAlive);
  }
});

test("native search error results omit secrets and provider answers", async () => {
  await assert.rejects(
    () =>
      runNativeSearch(
        { provider: "openai-codex", query: "q", limit: 3 },
        {
          resolveAuth: async () => ({
            apiKey: "codex-secret-token",
            headers: { Authorization: "Bearer codex-secret-token" },
          }),
          fetchImpl: async () =>
            new Response("unauthorized", { status: 401, headers: { "Content-Type": "text/plain" } }),
        },
      ),
    (error) => {
      const result = toSearchErrorResult("openai-codex", error);
      assert.equal(result.status, "error");
      assert.equal(result.error_kind, "unavailable");
      assert.equal("answer" in result, false);
      assert.equal(JSON.stringify(result).includes("codex-secret-token"), false);
      assert.equal(JSON.stringify(result).includes("Bearer"), false);
      return true;
    },
  );
});

test("runs a mocked Codex search with classic GPT-5.5 and Chat-compatible headers", async () => {
  let captured;
  const result = await runNativeSearch(
    { provider: "openai-codex", query: "latest public report", limit: 3 },
    {
      availableModelIds: ["gpt-5.5", "gpt-5.4-mini"],
      resolveAuth: async () => ({ apiKey: "codex-secret-token" }),
      fetchImpl: async (url, init) => {
        captured = { url: String(url), headers: init.headers, body: JSON.parse(init.body) };
        return new Response(CODEX_SEARCH_SSE, {
          status: 200,
          headers: { "Content-Type": "text/event-stream" },
        });
      },
    },
  );
  assert.equal(captured.body.model, "gpt-5.5");
  assert.deepEqual(captured.body.tool_choice, { type: "web_search" });
  assert.deepEqual(captured.body.tools, [
    { type: "web_search", search_context_size: "high" },
  ]);
  assert.equal(captured.body.reasoning.effort, "low");
  assert.equal(captured.headers["x-openai-internal-codex-responses-lite"], undefined);
  assert.match(captured.headers["user-agent"], /^pi \(.+\)$/);
  assert.equal(captured.headers.version, "0.144.1");
  assert.match(captured.headers["session-id"], /^[0-9a-f-]{36}$/);
  assert.equal(captured.headers["x-client-request-id"], captured.headers["session-id"]);
  assert.match(captured.url, /codex\/responses$/);
  assert.equal(result.status, "ok");
  assert.equal(result.sources[0].url, "https://example.com/article");
  assert.equal(JSON.stringify(result).includes("codex-secret-token"), false);
  assert.equal(JSON.stringify(result).includes("Provider synthesized answer"), false);
});

test("parses a non-SSE Codex JSON completed snapshot as hosted search", () => {
  const payload = {
    id: "resp_json_completed",
    object: "response",
    status: "completed",
    model: "gpt-5.6-luna",
    output: [
      {
        type: "web_search_preview_call",
        action: {
          sources: [{ url: "https://example.com/json-source", title: "JSON Source" }],
        },
      },
      {
        type: "message",
        content: [
          {
            type: "output_text",
            text: "Provider synthesized answer that must be discarded.",
            annotations: [
              {
                type: "url_citation",
                url: "https://example.com/json-source",
                title: "JSON Source",
                cited_text: "cited",
              },
            ],
          },
        ],
      },
    ],
    usage: { input_tokens: 9, output_tokens: 4, total_tokens: 13 },
  };
  const parsed = parseCodexSearchPayload(JSON.stringify(payload));
  const result = toSearchResult("openai-codex", parsed, 5);
  assert.equal(result.status, "ok");
  assert.equal(result.requestId, "resp_json_completed");
  assert.equal(result.sources[0].url, "https://example.com/json-source");
  assert.equal(result.sources[0].snippet, "cited");
  assert.equal(JSON.stringify(result).includes("Provider synthesized answer"), false);
});

test("parses NDJSON Codex events when SSE data: frames are omitted", () => {
  const ndjson = [
    JSON.stringify({
      type: "response.web_search_preview_call.completed",
      item_id: "ws_ndjson",
    }),
    JSON.stringify({
      type: "response.completed",
      response: {
        id: "resp_ndjson",
        model: "gpt-5.6-luna",
        output: [
          {
            type: "web_search_call",
            action: { sources: [{ url: "https://example.com/ndjson", title: "NDJSON" }] },
          },
        ],
      },
    }),
  ].join("\n");
  const parsed = parseCodexSearchPayload(ndjson);
  assert.equal(parsed.requestId, "resp_ndjson");
  assert.equal(parsed.sources[0].url, "https://example.com/ndjson");
  assert.equal(parseCodexJsonTransport(ndjson)[0].type, "response.web_search_preview_call.completed");
});

test("returns a completed Codex SSE frame without waiting for transport EOF", async () => {
  const sse = [
    `data: ${JSON.stringify({ type: "response.web_search_call.completed", item_id: "ws_open" })}`,
    "",
    `data: ${JSON.stringify({
      type: "response.completed",
      response: {
        id: "resp_open_stream",
        model: "gpt-5.5",
        output: [
          {
            type: "web_search_call",
            action: { sources: [{ url: "https://example.com/open", title: "Open" }] },
          },
        ],
      },
    })}`,
    "",
  ].join("\n");
  assert.equal(completedCodexTransportPrefix(`${sse}partial`), sse);

  const result = await runNativeSearch(
    { provider: "openai-codex", query: "bounded stream", limit: 3 },
    {
      availableModelIds: ["gpt-5.5"],
      resolveAuth: async () => ({ apiKey: "codex-secret-token" }),
      fetchImpl: async () =>
        new Response(
          new ReadableStream({
            start(controller) {
              controller.enqueue(new TextEncoder().encode(sse));
              // Intentionally never close: Codex may keep the SSE transport
              // alive after its terminal response event.
            },
          }),
          { status: 200, headers: { "Content-Type": "text/event-stream" } },
        ),
    },
  );
  assert.equal(result.status, "ok");
  assert.equal(result.sources[0].url, "https://example.com/open");
});

test("JSON Codex completions without a hosted search call stay invalid", () => {
  assert.throws(
    () =>
      parseCodexSearchPayload(
        JSON.stringify({
          id: "resp_json_no_search",
          object: "response",
          status: "completed",
          model: "gpt-5.6-luna",
          output: [{ type: "message", content: [{ type: "output_text", text: "stale" }] }],
        }),
      ),
    /without running web search/,
  );
});
