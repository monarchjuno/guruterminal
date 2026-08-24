import { constants as zlibConstants, zstdCompressSync } from "node:zlib";

export const DEFAULT_SEARCH_INSTRUCTIONS =
  "Find public sources for the given query. Prefer primary documents and recent reporting. Do not invent URLs.";
export const SEARCH_HARD_TIMEOUT_MS = 60_000;
const MAX_PROVIDER_BODY_BYTES = 1024 * 1024;

export class NativeSearchError extends Error {
  constructor(kind, message, options = {}) {
    super(message);
    this.name = "NativeSearchError";
    this.kind = kind;
    this.status = options.status;
    this.retryAfterMs = options.retryAfterMs;
  }
}

export function parseRetryAfterMs(headers, now = Date.now()) {
  if (!headers || typeof headers.get !== "function") return undefined;
  const retryAfterMs = headers.get("retry-after-ms");
  if (retryAfterMs !== null && retryAfterMs !== undefined) {
    const millis = Number(retryAfterMs);
    if (Number.isFinite(millis)) return Math.max(0, millis);
  }
  const retryAfter = headers.get("retry-after");
  if (!retryAfter) return undefined;
  const seconds = Number(retryAfter);
  if (Number.isFinite(seconds)) return Math.max(0, seconds * 1000);
  const date = Date.parse(retryAfter);
  if (!Number.isNaN(date)) return Math.max(0, date - now);
  return undefined;
}

export function classifyHttpFailure(status, body = "") {
  if (status === 429) return "rate_limited";
  if (status === 408 || status === 425 || status === 504) return "timeout";
  if (status >= 500 && status <= 599) return "transport";
  if (
    status === 401 ||
    status === 402 ||
    status === 403 ||
    /credits?\s*(?:exhausted|exceeded)|insufficient[_ ](?:quota|credits?)/iu.test(body)
  ) {
    return "unavailable";
  }
  if (/rate.?limit|too many requests/i.test(body)) return "rate_limited";
  return "provider";
}

export function throwHttpFailure(status, body, headers) {
  throw new NativeSearchError(classifyHttpFailure(status, body), `provider HTTP ${status}`, {
    status,
    retryAfterMs: status === 429 ? parseRetryAfterMs(headers) : undefined,
  });
}

function cleanSourceUrl(rawUrl) {
  try {
    const url = new URL(rawUrl);
    if (url.searchParams.get("utm_source") === "openai") url.searchParams.delete("utm_source");
    return url.toString();
  } catch {
    return String(rawUrl).replace(/[?&]utm_source=openai$/u, "");
  }
}

export function addSource(sources, source) {
  if (!source?.url || typeof source.url !== "string") return;
  const url = cleanSourceUrl(source.url.trim());
  if (!url) return;
  const existing = sources.find((candidate) => candidate.url === url);
  const title = source.title?.trim() || url;
  const snippet = source.snippet?.trim() || undefined;
  const publishedAt = source.publishedAt?.trim() || undefined;
  if (!existing) {
    sources.push({ title, url, snippet, publishedAt });
    return;
  }
  if (existing.title === existing.url && title !== url) existing.title = title;
  if (!existing.snippet && snippet) existing.snippet = snippet;
  if (!existing.publishedAt && publishedAt) existing.publishedAt = publishedAt;
}

export function parseSseJsonEvents(text) {
  const events = [];
  for (const block of String(text).split(/\r?\n\r?\n/)) {
    const dataLines = block
      .split(/\r?\n/)
      .filter((line) => line.startsWith("data:"))
      .map((line) => line.slice(5).trim());
    if (dataLines.length === 0) continue;
    const data = dataLines.join("\n").trim();
    if (!data || data === "[DONE]") continue;
    try {
      events.push(JSON.parse(data));
    } catch {
      throw new NativeSearchError("malformed", "provider SSE JSON is invalid");
    }
  }
  return events;
}

export function headerMap(auth) {
  const headers = {};
  for (const [key, value] of Object.entries(auth?.headers ?? {})) {
    if (typeof value === "string" && value.length > 0) headers[key.toLowerCase()] = value;
  }
  return headers;
}

function isPublicHttpUrl(value) {
  try {
    const parsed = new URL(value);
    return parsed.protocol === "https:" || parsed.protocol === "http:";
  } catch {
    return false;
  }
}

export async function readBoundedBody(response, completedPrefix) {
  const length = Number(response.headers.get("content-length"));
  if (Number.isFinite(length) && length > MAX_PROVIDER_BODY_BYTES) {
    throw new NativeSearchError("malformed", "provider response exceeded its size limit");
  }
  if (!response.body || typeof response.body.getReader !== "function") {
    const text = await response.text();
    if (Buffer.byteLength(text, "utf8") > MAX_PROVIDER_BODY_BYTES) {
      throw new NativeSearchError("malformed", "provider response exceeded its size limit");
    }
    return text;
  }
  const reader = response.body.getReader();
  const chunks = [];
  let total = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > MAX_PROVIDER_BODY_BYTES) {
      await reader.cancel().catch(() => undefined);
      throw new NativeSearchError("malformed", "provider response exceeded its size limit");
    }
    chunks.push(value);
    if (completedPrefix) {
      const text = Buffer.concat(chunks.map((chunk) => Buffer.from(chunk))).toString("utf8");
      const prefix = completedPrefix(text);
      if (prefix !== undefined) {
        await reader.cancel().catch(() => undefined);
        return prefix;
      }
    }
  }
  return Buffer.concat(chunks.map((chunk) => Buffer.from(chunk))).toString("utf8");
}

export function searchQueryText(request) {
  const parts = [request.query.trim()];
  if (request.recency) parts.push(`Prefer sources published within the last ${request.recency}.`);
  if (request.include_domains?.length) {
    parts.push(`Include only sources from: ${request.include_domains.join(", ")}.`);
  }
  if (request.exclude_domains?.length) {
    parts.push(`Exclude sources from: ${request.exclude_domains.join(", ")}.`);
  }
  return parts.join(" ");
}

function sanitizeSources(sources, limit) {
  const seen = new Set();
  const sanitized = [];
  for (const source of sources) {
    if (!isPublicHttpUrl(source.url) || seen.has(source.url)) continue;
    seen.add(source.url);
    sanitized.push({
      title: String(source.title ?? "Untitled source").slice(0, 512),
      url: source.url,
      snippet: source.snippet ? String(source.snippet).slice(0, 2000) : undefined,
      publishedAt: source.publishedAt ? String(source.publishedAt).slice(0, 128) : undefined,
    });
    if (sanitized.length >= limit) break;
  }
  return sanitized;
}

function sanitizeUsage(usage) {
  if (!usage || typeof usage !== "object") return undefined;
  const next = {};
  for (const key of ["inputTokens", "outputTokens", "totalTokens", "searchRequests"]) {
    if (typeof usage[key] === "number" && Number.isFinite(usage[key])) next[key] = usage[key];
  }
  return Object.keys(next).length > 0 ? next : undefined;
}

export function toSearchResult(provider, parsed, limit) {
  return {
    type: "search",
    provider,
    status: "ok",
    sources: sanitizeSources(parsed.sources ?? [], limit),
    model: typeof parsed.model === "string" ? parsed.model.slice(0, 128) : undefined,
    requestId: typeof parsed.requestId === "string" ? parsed.requestId.slice(0, 128) : undefined,
    usage: sanitizeUsage(parsed.usage),
    searchRequestCount:
      typeof parsed.searchRequestCount === "number" ? parsed.searchRequestCount : undefined,
  };
}

export function toSearchErrorResult(provider, error) {
  const kind =
    error instanceof NativeSearchError
      ? error.kind
      : error?.name === "AbortError" || error?.name === "TimeoutError"
        ? "timeout"
        : "provider";
  return {
    type: "search",
    provider,
    status: "error",
    error_kind: kind,
    retry_after_ms:
      error instanceof NativeSearchError && typeof error.retryAfterMs === "number"
        ? error.retryAfterMs
        : undefined,
  };
}

export function assertSearchResultSafe(value, secrets = []) {
  const encoded = JSON.stringify(value);
  if (Object.prototype.hasOwnProperty.call(value, "answer")) {
    throw new NativeSearchError("malformed", "search result must not include provider answer");
  }
  if (Object.prototype.hasOwnProperty.call(value, "credential")) {
    throw new NativeSearchError("malformed", "search result must not include credentials");
  }
  for (const secret of secrets) {
    if (secret && encoded.includes(secret)) {
      throw new NativeSearchError("malformed", "search result leaked a credential");
    }
  }
}

export async function postJson({
  fetchImpl,
  url,
  headers,
  body,
  signal,
  completedPrefix,
  compress = false,
}) {
  const requestHeaders = { ...headers };
  const bodyJson = JSON.stringify(body);
  const encodedBody = compress
    ? zstdCompressSync(bodyJson, {
        params: { [zlibConstants.ZSTD_c_compressionLevel]: 3 },
      })
    : bodyJson;
  if (compress) requestHeaders["content-encoding"] = "zstd";
  let response;
  try {
    response = await fetchImpl(url, {
      method: "POST",
      headers: requestHeaders,
      body: encodedBody,
      signal,
    });
  } catch (error) {
    if (error?.name === "AbortError" || error?.name === "TimeoutError") {
      throw new NativeSearchError("timeout", "provider request timed out");
    }
    throw new NativeSearchError("transport", "provider transport failed");
  }
  const text = await readBoundedBody(response, completedPrefix);
  if (!response.ok) throwHttpFailure(response.status, text, response.headers);
  return text;
}
