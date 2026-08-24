import {
  addSource,
  DEFAULT_SEARCH_INSTRUCTIONS,
  NativeSearchError,
  parseSseJsonEvents,
  postJson,
  readBoundedBody,
  searchQueryText,
  throwHttpFailure,
} from "./common.mjs";

// Hosted-search parsing is derived from Oh My Pi. See ../guruterminal-native-search.mjs
// for the pinned upstream revision and MIT notice.
export const ANTHROPIC_SEARCH_MODELS = Object.freeze([
  "claude-haiku-4-5",
  "claude-sonnet-4-5",
  "claude-sonnet-4-6",
]);

function parsePageAge(pageAge) {
  if (!pageAge || typeof pageAge !== "string") return undefined;
  return pageAge;
}

function toolErrorCode(block) {
  if (!block || typeof block !== "object") return undefined;
  const content = block.content;
  if (content && typeof content === "object" && !Array.isArray(content)) {
    if (content.type === "web_search_tool_result_error" || content.type === "tool_result_error") {
      return content.error_code ?? "unknown";
    }
  }
  if (Array.isArray(content)) {
    const error = content.find(
      (item) => item?.type === "web_search_tool_result_error" || item?.type === "web_search_tool_error",
    );
    if (error) return error.error_code ?? "unknown";
  }
  return block.is_error === true ? "unknown" : undefined;
}

function classifyToolError(code) {
  if (code === "too_many_requests") return "rate_limited";
  if (code === "unavailable") return "unavailable";
  return "provider";
}

function isWebSearchTool(name) {
  const normalized = String(name ?? "")
    .replace(/^claude_code_/, "")
    .toLowerCase()
    .replace(/[_-]/gu, "");
  return normalized === "websearch";
}

export function parseAnthropicSearchResponse(response) {
  if (!response || typeof response !== "object" || !Array.isArray(response.content)) {
    throw new NativeSearchError("malformed", "Anthropic search response is invalid");
  }
  const sources = [];
  let searchRequestCount = 0;
  let invoked = false;
  for (const block of response.content) {
    if (block.type === "server_tool_use" && isWebSearchTool(block.name)) {
      invoked = true;
      searchRequestCount += 1;
      continue;
    }
    if (block.type === "web_search_tool_result") {
      invoked = true;
      const errorCode = toolErrorCode(block);
      if (errorCode) {
        throw new NativeSearchError(
          classifyToolError(errorCode),
          `Anthropic web search tool returned an error (${errorCode})`,
        );
      }
      for (const result of Array.isArray(block.content) ? block.content : []) {
        if (result?.type === "web_search_result" && result.url) {
          addSource(sources, {
            title: result.title ?? result.url,
            url: result.url,
            publishedAt: parsePageAge(result.page_age),
          });
        }
      }
      continue;
    }
    if (block.type === "text" && Array.isArray(block.citations)) {
      for (const citation of block.citations) {
        if (citation?.url) {
          addSource(sources, {
            title: citation.title ?? citation.url,
            url: citation.url,
            snippet: citation.cited_text,
          });
        }
      }
    }
  }
  if (!invoked) {
    throw new NativeSearchError(
      "no_search_tool",
      "Anthropic returned a completion without running web search",
    );
  }
  const usage = response.usage
    ? {
        inputTokens: response.usage.input_tokens,
        outputTokens: response.usage.output_tokens,
        searchRequests: response.usage.server_tool_use?.web_search_requests,
      }
    : undefined;
  return {
    sources,
    model: response.model,
    requestId: response.id,
    usage,
    searchRequestCount: usage?.searchRequests ?? searchRequestCount,
  };
}

function classifyStreamError(error) {
  const detail = `${error?.type ?? ""} ${error?.message ?? ""}`.toLowerCase();
  if (/rate.?limit|too many requests|quota|\b429\b/u.test(detail)) return "rate_limited";
  if (/authentication|unauthori[sz]ed|permission|forbidden|\b40[13]\b/u.test(detail)) {
    return "unavailable";
  }
  if (/timeout|timed out/u.test(detail)) return "timeout";
  return "provider";
}

function mergeUsage(target, usage) {
  if (!usage || typeof usage !== "object") return;
  for (const key of ["input_tokens", "output_tokens"]) {
    if (typeof usage[key] === "number") target[key] = usage[key];
  }
  const requests = usage.server_tool_use?.web_search_requests;
  if (typeof requests === "number") target.server_tool_use = { web_search_requests: requests };
}

export function parseAnthropicSearchEvents(events) {
  const blocks = new Map();
  const usage = {};
  let model;
  let requestId;
  for (const event of events) {
    if (!event || typeof event !== "object") continue;
    if (event.type === "error") {
      const error = event.error && typeof event.error === "object" ? event.error : event;
      throw new NativeSearchError(
        classifyStreamError(error),
        `Anthropic error: ${error.message || error.type || "Unknown error"}`,
      );
    }
    if (event.type === "message_start") {
      requestId = event.message?.id ?? requestId;
      model = event.message?.model ?? model;
      mergeUsage(usage, event.message?.usage);
      continue;
    }
    if (event.type === "content_block_start" && Number.isInteger(event.index)) {
      if (event.content_block && typeof event.content_block === "object") {
        blocks.set(event.index, structuredClone(event.content_block));
      }
      continue;
    }
    if (event.type === "content_block_delta" && Number.isInteger(event.index)) {
      const block = blocks.get(event.index);
      const delta = event.delta;
      if (!block || !delta || typeof delta !== "object") continue;
      if (delta.type === "text_delta" && typeof delta.text === "string") {
        block.text = `${block.text ?? ""}${delta.text}`;
      } else if (delta.type === "citations_delta" && delta.citation) {
        block.citations = [...(Array.isArray(block.citations) ? block.citations : []), delta.citation];
      }
      continue;
    }
    if (event.type === "message_delta") mergeUsage(usage, event.usage);
  }
  return parseAnthropicSearchResponse({
    id: requestId,
    model,
    content: [...blocks.entries()]
      .sort(([left], [right]) => left - right)
      .map(([, block]) => block),
    usage,
  });
}

export function parseAnthropicSearchPayload(text) {
  const trimmed = String(text).trim();
  if (!trimmed) {
    throw new NativeSearchError("malformed", "Anthropic search response is empty");
  }
  try {
    return parseAnthropicSearchResponse(JSON.parse(trimmed));
  } catch (error) {
    if (error instanceof NativeSearchError && error.kind !== "malformed") throw error;
  }
  const events = parseSseJsonEvents(trimmed);
  if (events.length === 0) {
    throw new NativeSearchError("malformed", "Anthropic search response is invalid");
  }
  return parseAnthropicSearchEvents(events);
}

export function completedAnthropicTransportPrefix(text) {
  const raw = String(text);
  for (const match of raw.matchAll(/(?:^|\r?\n)data:\s*(.*?)\r?\n/g)) {
    const data = match[1].trim();
    if (!data) continue;
    try {
      const event = JSON.parse(data);
      if (event?.type === "message_stop" || event?.type === "error") {
        return raw.slice(0, match.index + match[0].length);
      }
    } catch {
      // Keep buffering until a complete JSON data line arrives.
    }
  }
  return undefined;
}

export async function runAnthropicSearchThroughPi(ctx, { model, request, signal }) {
  const selected = ctx.modelRegistry.find("anthropic", model);
  if (!selected) throw new NativeSearchError("unavailable", "Pi search model is unavailable");
  let capturedResponse;
  let capturedStatus;
  let capturedHeaders;
  const captureFetch = async (url, init) => {
    const response = await fetch(url, init);
    if (!response.body) {
      throw new NativeSearchError("malformed", "Anthropic search response had no body");
    }
    capturedStatus = response.status;
    capturedHeaders = response.headers;
    const [providerBody, captureBody] = response.body.tee();
    capturedResponse = readBoundedBody(
      new Response(captureBody, {
        status: response.status,
        statusText: response.statusText,
        headers: response.headers,
      }),
      completedAnthropicTransportPrefix,
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
        systemPrompt: DEFAULT_SEARCH_INSTRUCTIONS,
        messages: [{
          role: "user",
          content: [{ type: "text", text: searchQueryText(request) }],
          timestamp: Date.now(),
        }],
        tools: [],
      },
      {
        signal,
        timeoutMs: 55_000,
        fetch: captureFetch,
        onPayload: (basePayload) => buildAnthropicBody(request, model, basePayload),
      },
    );
  } catch (error) {
    if (error instanceof NativeSearchError) throw error;
    if (capturedResponse && typeof capturedStatus === "number" && capturedStatus >= 400) {
      const errorBody = await capturedResponse.catch(() => "");
      throwHttpFailure(capturedStatus, errorBody, capturedHeaders);
    }
    throw new NativeSearchError("transport", "Pi Anthropic search transport failed");
  }
  if (!capturedResponse) {
    throw new NativeSearchError("malformed", "Pi Anthropic search response was unavailable");
  }
  return capturedResponse;
}

export function buildAnthropicBody(request, model, basePayload = {}) {
  const tool = { type: "web_search_20250305", name: "web_search" };
  if (request.include_domains?.length) tool.allowed_domains = request.include_domains.slice(0, 10);
  else if (request.exclude_domains?.length) {
    tool.blocked_domains = request.exclude_domains.slice(0, 10);
  }
  return {
    ...(basePayload && typeof basePayload === "object" ? basePayload : {}),
    model,
    max_tokens: 4096,
    messages: [{ role: "user", content: searchQueryText(request) }],
    tools: [tool],
    system:
      basePayload && typeof basePayload === "object" && basePayload.system
        ? basePayload.system
        : DEFAULT_SEARCH_INSTRUCTIONS,
  };
}

async function search({ request, model, auth, headers, deps, signal }) {
  const token = auth.apiKey;
  if (token?.includes("sk-ant-oat")) {
    delete headers["x-api-key"];
    headers.authorization ||= `Bearer ${token}`;
    headers["anthropic-beta"] ||= "claude-code-20250219,oauth-2025-04-20";
    headers["anthropic-dangerous-direct-browser-access"] ||= "true";
    headers["x-app"] ||= "cli";
  }
  headers["anthropic-version"] ||= "2023-06-01";
  const base = (auth.baseUrl || "https://api.anthropic.com").replace(/\/+$/, "");
  const url = base.endsWith("/v1/messages") ? base : `${base}/v1/messages`;
  const body = buildAnthropicBody(request, model);
  const text = deps.anthropicTransport
    ? await deps.anthropicTransport({ body, model, request, signal })
    : await postJson({
        fetchImpl: deps.fetchImpl ?? fetch,
        url,
        headers,
        body,
        signal,
      });
  return parseAnthropicSearchPayload(text);
}

export const anthropicSearchAdapter = Object.freeze({
  id: "anthropic",
  modelIds: ANTHROPIC_SEARCH_MODELS,
  search,
});
