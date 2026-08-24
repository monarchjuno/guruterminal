import { randomUUID } from "node:crypto";
import { release } from "node:os";

import {
  addSource,
  DEFAULT_SEARCH_INSTRUCTIONS,
  NativeSearchError,
  parseSseJsonEvents,
  postJson,
  readBoundedBody,
  searchQueryText,
} from "./common.mjs";

// Hosted-search parsing is derived from Oh My Pi. See ../guruterminal-native-search.mjs
// for the pinned upstream revision and MIT notice.
export const CODEX_SEARCH_MODELS = Object.freeze([
  "gpt-5.5",
  "gpt-5.4-mini",
  "gpt-5.4",
  "gpt-5.6-luna",
  "gpt-5.6-terra",
  "gpt-5.6-sol",
]);
const RESPONSES_LITE_MODELS = new Set(["gpt-5.6-luna", "gpt-5.6-terra", "gpt-5.6-sol"]);
const HOSTED_SEARCH_TOOL = Object.freeze({ type: "web_search", search_context_size: "high" });
const JWT_CLAIM_PATH = "https://api.openai.com/auth";

export function isCodexResponsesLiteModel(modelId) {
  return RESPONSES_LITE_MODELS.has(modelId);
}

function completedEvent(response) {
  return { type: "response.completed", response };
}

function parseJsonObject(parsed) {
  if (Array.isArray(parsed)) return parsed;
  if (!parsed || typeof parsed !== "object") {
    throw new NativeSearchError("malformed", "Codex search response is invalid");
  }
  if (typeof parsed.type === "string") return [parsed];
  if (
    Array.isArray(parsed.output) ||
    parsed.object === "response" ||
    typeof parsed.status === "string" ||
    typeof parsed.id === "string"
  ) {
    return [completedEvent(parsed)];
  }
  throw new NativeSearchError("malformed", "Codex search response is invalid");
}

function parseNdjsonEvents(text) {
  const events = [];
  for (const line of String(text).split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith(":")) continue;
    try {
      events.push(JSON.parse(trimmed));
    } catch {
      throw new NativeSearchError("malformed", "Codex search response is invalid");
    }
  }
  if (events.length === 0) {
    throw new NativeSearchError("malformed", "Codex search response is invalid");
  }
  return events;
}

export function parseCodexJsonTransport(text) {
  try {
    return parseJsonObject(JSON.parse(text));
  } catch (error) {
    if (error instanceof NativeSearchError) throw error;
    return parseNdjsonEvents(text);
  }
}

export function parseCodexSearchPayload(text) {
  const trimmed = String(text).trim();
  if (!trimmed) {
    throw new NativeSearchError("malformed", "Codex search response is empty");
  }
  const sseEvents = parseSseJsonEvents(trimmed);
  if (sseEvents.length > 0) return parseCodexSearchEvents(sseEvents);
  return parseCodexSearchEvents(parseCodexJsonTransport(trimmed));
}

function extractError(rawEvent) {
  const candidates = [rawEvent, rawEvent?.error, rawEvent?.response?.error];
  let code = "";
  let message = "";
  for (const candidate of candidates) {
    if (!candidate || typeof candidate !== "object") continue;
    if (!code && typeof candidate.code === "string" && candidate.code) code = candidate.code;
    if (!message && typeof candidate.message === "string" && candidate.message) {
      message = candidate.message;
    }
  }
  return { code, message };
}

function classifyError(code, message) {
  const detail = `${code} ${message}`.toLowerCase();
  if (/rate[- ]?limit|too many requests|quota|\b429\b/u.test(detail)) return "rate_limited";
  if (/unauthori[sz]ed|\b401\b|forbidden|\b403\b/u.test(detail)) return "unavailable";
  if (/timeout|timed out/u.test(detail)) return "timeout";
  return "provider";
}

function isHostedSearchItem(item) {
  const type = typeof item?.type === "string" ? item.type : "";
  return type === "web_search_call" || type === "web_search_preview_call";
}

function collectOutputItem(item, sources) {
  if (!item || typeof item !== "object") return false;
  let invoked = false;
  if (isHostedSearchItem(item)) {
    invoked = true;
    for (const group of [item.action?.sources, item.sources, item.results]) {
      for (const source of group ?? []) {
        const url = source?.url ?? source?.source_website_url;
        if (!url) continue;
        addSource(sources, { title: source.title ?? source.caption ?? url, url });
      }
    }
  }
  if (item.type === "message" && Array.isArray(item.content)) {
    for (const part of item.content) {
      for (const annotation of part?.annotations ?? []) {
        if (annotation?.type === "url_citation" && annotation.url) {
          addSource(sources, {
            title: annotation.title ?? annotation.url,
            url: annotation.url,
            snippet: annotation.cited_text ?? annotation.snippet,
          });
        }
      }
    }
  }
  return invoked;
}

export function parseCodexSearchEvents(events) {
  const sources = [];
  let model;
  let requestId;
  let usage;
  let webSearchInvoked = false;

  for (const rawEvent of events) {
    const eventType = typeof rawEvent?.type === "string" ? rawEvent.type : "";
    if (!eventType) continue;
    if (eventType.startsWith("response.web_search")) webSearchInvoked = true;
    if (eventType === "error") {
      const { code, message } = extractError(rawEvent);
      throw new NativeSearchError(
        classifyError(code, message),
        `Codex error (${code}): ${message || "Unknown error"}`,
      );
    }
    if (eventType === "response.failed") {
      const { code, message } = extractError(rawEvent);
      throw new NativeSearchError(
        classifyError(code, message),
        code
          ? `Codex request failed (${code}): ${message || "Request failed"}`
          : `Codex request failed: ${message || "Request failed"}`,
      );
    }
    if (eventType === "response.created") {
      requestId = rawEvent.response?.id ?? requestId;
      model = rawEvent.response?.model ?? model;
      continue;
    }
    if (eventType === "response.output_item.done" || eventType === "response.output_item.added") {
      if (collectOutputItem(rawEvent.item, sources)) webSearchInvoked = true;
      continue;
    }
    if (eventType === "response.completed" || eventType === "response.done") {
      const response = rawEvent.response;
      model = response?.model ?? model;
      requestId = response?.id ?? requestId;
      if (response?.usage) {
        usage = {
          inputTokens: response.usage.input_tokens,
          outputTokens: response.usage.output_tokens,
          totalTokens: response.usage.total_tokens,
        };
      }
      for (const item of Array.isArray(response?.output) ? response.output : []) {
        if (collectOutputItem(item, sources)) webSearchInvoked = true;
      }
    }
  }
  if (!webSearchInvoked) {
    throw new NativeSearchError(
      "no_search_tool",
      "Codex returned a completion without running web search",
    );
  }
  return { sources, model, requestId, usage, searchRequestCount: 1 };
}

export function chatgptAccountId(token) {
  try {
    const parts = String(token).split(".");
    if (parts.length < 2) return undefined;
    const payload = JSON.parse(Buffer.from(parts[1], "base64url").toString("utf8"));
    return payload?.[JWT_CLAIM_PATH]?.chatgpt_account_id;
  } catch {
    return undefined;
  }
}

function resolveUrl(baseUrl) {
  const raw = (baseUrl && String(baseUrl).trim()) || "https://chatgpt.com/backend-api";
  const normalized = raw.replace(/\/+$/, "");
  if (normalized.endsWith("/codex/responses")) return normalized;
  if (normalized.endsWith("/codex")) return `${normalized}/responses`;
  return `${normalized}/codex/responses`;
}

function isTerminalEvent(event) {
  const type = typeof event?.type === "string" ? event.type : "";
  if (["error", "response.failed", "response.completed", "response.done", "response.incomplete"].includes(type)) {
    return true;
  }
  return (
    event?.object === "response" &&
    ["completed", "failed", "incomplete", "cancelled"].includes(event.status)
  );
}

export function completedCodexTransportPrefix(text) {
  const raw = String(text);
  for (const match of raw.matchAll(/(?:^|\r?\n)data:\s*(.*?)\r?\n/g)) {
    const data = match[1].trim();
    if (!data || data === "[DONE]") continue;
    try {
      if (isTerminalEvent(JSON.parse(data))) return raw.slice(0, match.index + match[0].length);
    } catch {
      // Keep buffering until a complete JSON data line arrives.
    }
  }
  let completeSseEnd = 0;
  for (const match of raw.matchAll(/\r?\n\r?\n/g)) completeSseEnd = match.index + match[0].length;
  if (completeSseEnd > 0) {
    const prefix = raw.slice(0, completeSseEnd);
    try {
      if (parseSseJsonEvents(prefix).some(isTerminalEvent)) return prefix;
    } catch {
      // The final parser reports complete malformed frames after transport close.
    }
  }
  try {
    const parsed = JSON.parse(raw);
    const events = Array.isArray(parsed) ? parsed : [parsed];
    if (events.some(isTerminalEvent)) return raw;
  } catch {
    // A streaming JSON object or NDJSON line may still be incomplete.
  }
  let ndjsonEnd = 0;
  for (const match of raw.matchAll(/.*(?:\r?\n|$)/g)) {
    const line = match[0];
    if (!line) continue;
    const trimmed = line.trim();
    ndjsonEnd = match.index + line.length;
    if (!trimmed || trimmed.startsWith(":") || !line.endsWith("\n")) continue;
    try {
      if (isTerminalEvent(JSON.parse(trimmed))) return raw.slice(0, ndjsonEnd);
    } catch {
      return undefined;
    }
  }
  return undefined;
}

export async function runCodexSearchThroughPi(ctx, { body, model, signal }) {
  const selected = ctx.modelRegistry.find("openai-codex", model);
  if (!selected) throw new NativeSearchError("unavailable", "Pi search model is unavailable");
  let capturedResponse;
  const captureFetch = async (url, init) => {
    const response = await fetch(url, init);
    if (!response.body) {
      throw new NativeSearchError("malformed", "Codex search response had no body");
    }
    const [providerBody, captureBody] = response.body.tee();
    capturedResponse = readBoundedBody(
      new Response(captureBody, {
        status: response.status,
        statusText: response.statusText,
        headers: response.headers,
      }),
      completedCodexTransportPrefix,
    );
    return new Response(providerBody, {
      status: response.status,
      statusText: response.statusText,
      headers: response.headers,
    });
  };
  try {
    await ctx.modelRegistry.complete(
      selected,
      {
        systemPrompt: "Run the supplied bounded hosted-search request.",
        messages: [{
          role: "user",
          content: [{ type: "text", text: "Run the supplied search request." }],
          timestamp: Date.now(),
        }],
        tools: [],
      },
      {
        signal,
        timeoutMs: 55_000,
        transport: "sse",
        fetch: captureFetch,
        onPayload: () => body,
      },
    );
  } catch (error) {
    if (error instanceof NativeSearchError) throw error;
    throw new NativeSearchError("transport", "Pi Codex search transport failed");
  }
  if (!capturedResponse) {
    throw new NativeSearchError("malformed", "Pi Codex search response was unavailable");
  }
  return capturedResponse;
}

export function buildCodexBody(request, model) {
  return {
    model,
    stream: true,
    store: false,
    include: ["web_search_call.action.sources"],
    parallel_tool_calls: true,
    input: [{
      type: "message",
      role: "user",
      content: [{ type: "input_text", text: searchQueryText(request) }],
    }],
    tools: [HOSTED_SEARCH_TOOL],
    tool_choice: { type: "web_search" },
    instructions: DEFAULT_SEARCH_INSTRUCTIONS,
    reasoning: { effort: "low" },
  };
}

async function search({ request, model, auth, headers, deps, signal }) {
  const token = auth.apiKey || headers.authorization?.replace(/^Bearer\s+/i, "");
  const accountId = headers["chatgpt-account-id"] || chatgptAccountId(token);
  if (accountId) headers["chatgpt-account-id"] = accountId;
  headers["openai-beta"] ||= "responses=experimental";
  headers.originator ||= "pi";
  headers.version ||= "0.144.1";
  headers["user-agent"] ||= `pi (${process.platform} ${release()}; ${process.arch})`;
  const requestId = randomUUID();
  headers["session-id"] ||= requestId;
  headers["x-client-request-id"] ||= requestId;
  headers.accept = "text/event-stream";
  const body = buildCodexBody(request, model);
  const text = deps.codexTransport
    ? await deps.codexTransport({ body, model, signal })
    : await postJson({
        fetchImpl: deps.fetchImpl ?? fetch,
        url: resolveUrl(auth.baseUrl),
        headers,
        body,
        signal,
        completedPrefix: completedCodexTransportPrefix,
        compress: !deps.fetchImpl,
      });
  return parseCodexSearchPayload(text);
}

export const codexSearchAdapter = Object.freeze({
  id: "openai-codex",
  modelIds: CODEX_SEARCH_MODELS,
  search,
});
