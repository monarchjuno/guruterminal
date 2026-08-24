// Provider wire contracts are ported from Oh My Pi (can1357/oh-my-pi) commit
// 76a294cb19bfded1e32e2111f1f729129595bf5e.
//
// MIT License
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

import {
  headerMap,
  NativeSearchError,
  SEARCH_HARD_TIMEOUT_MS,
  toSearchResult,
} from "./native-search/common.mjs";
import { anthropicSearchAdapter } from "./native-search/anthropic.mjs";
import { codexSearchAdapter } from "./native-search/codex.mjs";
import { xaiSearchAdapter } from "./native-search/xai.mjs";

export {
  assertSearchResultSafe,
  classifyHttpFailure,
  NativeSearchError,
  parseRetryAfterMs,
  parseSseJsonEvents,
  readBoundedBody,
  toSearchErrorResult,
  toSearchResult,
} from "./native-search/common.mjs";
export {
  buildCodexBody,
  chatgptAccountId,
  completedCodexTransportPrefix,
  isCodexResponsesLiteModel,
  parseCodexJsonTransport,
  parseCodexSearchEvents,
  parseCodexSearchPayload,
  runCodexSearchThroughPi,
} from "./native-search/codex.mjs";
export {
  buildAnthropicBody,
  completedAnthropicTransportPrefix,
  parseAnthropicSearchEvents,
  parseAnthropicSearchPayload,
  parseAnthropicSearchResponse,
  runAnthropicSearchThroughPi,
} from "./native-search/anthropic.mjs";
export { buildXaiBody, parseXaiSearchResponse } from "./native-search/xai.mjs";

const ADAPTERS = Object.freeze({
  [codexSearchAdapter.id]: codexSearchAdapter,
  [anthropicSearchAdapter.id]: anthropicSearchAdapter,
  [xaiSearchAdapter.id]: xaiSearchAdapter,
});

export const NATIVE_SEARCH_PROVIDERS = Object.freeze(Object.keys(ADAPTERS));
export const SEARCH_MODELS = Object.freeze(
  Object.fromEntries(
    Object.entries(ADAPTERS).map(([providerId, adapter]) => [providerId, adapter.modelIds]),
  ),
);

export function isNativeSearchProvider(providerId) {
  return Object.hasOwn(ADAPTERS, providerId);
}

export function selectSearchModel(providerId, availableIds = []) {
  const allowlist = ADAPTERS[providerId]?.modelIds ?? [];
  const available = new Set(availableIds);
  return allowlist.find((id) => available.has(id)) ?? allowlist[0];
}

function authenticatedHeaders(providerId, auth) {
  const headers = headerMap(auth);
  if (auth.apiKey && !headers.authorization && !headers["x-api-key"]) {
    if (providerId === "anthropic") headers["x-api-key"] = auth.apiKey;
    else headers.authorization = `Bearer ${auth.apiKey}`;
  }
  headers["content-type"] ||= "application/json";
  return headers;
}

export async function runNativeSearch(request, deps) {
  const adapter = ADAPTERS[request.provider];
  if (!adapter) {
    throw new NativeSearchError("unavailable", "search provider is not allowlisted");
  }
  if (
    typeof request.query !== "string" ||
    request.query.trim().length === 0 ||
    request.query.length > 4_096
  ) {
    throw new NativeSearchError("malformed", "search query is invalid");
  }
  if (!Number.isInteger(request.limit) || request.limit < 1 || request.limit > 10) {
    throw new NativeSearchError("malformed", "search limit is invalid");
  }
  if (request.recency !== undefined && !["day", "week", "month", "year"].includes(request.recency)) {
    throw new NativeSearchError("malformed", "search recency is invalid");
  }
  for (const domains of [request.include_domains, request.exclude_domains]) {
    if (
      domains !== undefined &&
      (!Array.isArray(domains) ||
        domains.length > 10 ||
        domains.some(
          (domain) =>
            typeof domain !== "string" ||
            domain.length < 1 ||
            domain.length > 253 ||
            !/^[a-z0-9.-]+$/u.test(domain),
        ))
    ) {
      throw new NativeSearchError("malformed", "search domains are invalid");
    }
  }
  const timeoutSignal = AbortSignal.timeout(deps.timeoutMs ?? SEARCH_HARD_TIMEOUT_MS);
  const signal = deps.signal ? AbortSignal.any([deps.signal, timeoutSignal]) : timeoutSignal;
  const auth = await deps.resolveAuth(adapter.id);
  if (!auth) throw new NativeSearchError("unavailable", "provider credentials are unavailable");
  const model = request.model || selectSearchModel(adapter.id, deps.availableModelIds ?? []);
  let parsed;
  try {
    parsed = await adapter.search({
      request,
      model,
      auth,
      headers: authenticatedHeaders(adapter.id, auth),
      deps,
      signal,
    });
  } catch (error) {
    if (deps.signal?.aborted && error instanceof NativeSearchError && error.kind === "timeout") {
      throw new NativeSearchError("cancelled", "provider request was cancelled");
    }
    throw error;
  }
  return toSearchResult(adapter.id, parsed, request.limit);
}
