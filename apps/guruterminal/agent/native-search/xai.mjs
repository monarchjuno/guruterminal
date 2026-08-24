import {
  addSource,
  DEFAULT_SEARCH_INSTRUCTIONS,
  NativeSearchError,
  postJson,
  searchQueryText,
} from "./common.mjs";

// Hosted-search parsing is derived from Oh My Pi. See ../guruterminal-native-search.mjs
// for the pinned upstream revision and MIT notice.
export const XAI_SEARCH_MODELS = Object.freeze(["grok-4.5"]);

function collectAnnotationSources(annotations, sources) {
  if (!Array.isArray(annotations)) return;
  for (const annotation of annotations) {
    if (!annotation || annotation.type !== "url_citation" || typeof annotation.url !== "string") {
      continue;
    }
    addSource(sources, {
      title: annotation.title ?? annotation.url,
      url: annotation.url,
      snippet: annotation.cited_text ?? annotation.text ?? annotation.snippet,
    });
  }
}

export function parseXaiSearchResponse(response) {
  if (!response || typeof response !== "object") {
    throw new NativeSearchError("malformed", "xAI search response is invalid");
  }
  const sources = [];
  let invoked = false;
  collectAnnotationSources(response.annotations, sources);
  for (const item of Array.isArray(response.output) ? response.output : []) {
    if (!item || typeof item !== "object") continue;
    collectAnnotationSources(item.annotations, sources);
    for (const part of Array.isArray(item.content) ? item.content : []) {
      if (part && typeof part === "object") collectAnnotationSources(part.annotations, sources);
    }
    if (item.type !== "web_search_call") continue;
    invoked = true;
    for (const group of [item.action?.sources, item.sources, item.results]) {
      if (!Array.isArray(group)) continue;
      for (const source of group) {
        const url = source?.url ?? source?.source_website_url;
        if (typeof url !== "string") continue;
        addSource(sources, { title: source.title ?? source.caption ?? url, url });
      }
    }
  }
  for (const url of Array.isArray(response.citations) ? response.citations : []) {
    if (typeof url === "string") addSource(sources, { title: url, url });
  }
  if (!invoked) {
    throw new NativeSearchError(
      "no_search_tool",
      "xAI returned a completion without running web search",
    );
  }
  const usage = response.usage
    ? {
        inputTokens: response.usage.input_tokens ?? response.usage.inputTokens,
        outputTokens: response.usage.output_tokens ?? response.usage.outputTokens,
        totalTokens: response.usage.total_tokens ?? response.usage.totalTokens,
      }
    : undefined;
  return {
    sources,
    model: response.model,
    requestId: response.id,
    usage,
    searchRequestCount: 1,
  };
}

export function buildXaiBody(request, model) {
  const tool = { type: "web_search" };
  if (request.include_domains?.length) {
    tool.filters = { allowed_domains: request.include_domains.slice(0, 5) };
  } else if (request.exclude_domains?.length) {
    tool.filters = { excluded_domains: request.exclude_domains.slice(0, 5) };
  }
  return {
    model,
    input: [
      { role: "system", content: DEFAULT_SEARCH_INSTRUCTIONS },
      { role: "user", content: searchQueryText(request) },
    ],
    tools: [tool],
    include: ["web_search_call.action.sources"],
    reasoning: { effort: "low" },
  };
}

async function search({ request, model, auth, headers, deps, signal }) {
  const base = (auth.baseUrl || "https://api.x.ai/v1").replace(/\/+$/, "");
  const url = base.endsWith("/responses") ? base : `${base}/responses`;
  const text = await postJson({
    fetchImpl: deps.fetchImpl ?? fetch,
    url,
    headers,
    body: buildXaiBody(request, model),
    signal,
  });
  try {
    return parseXaiSearchResponse(JSON.parse(text));
  } catch (error) {
    if (error instanceof NativeSearchError) throw error;
    throw new NativeSearchError("malformed", "xAI search response is invalid");
  }
}

export const xaiSearchAdapter = Object.freeze({
  id: "xai",
  modelIds: XAI_SEARCH_MODELS,
  search,
});
