export const PROTOCOL = "guruterminal-compute/2";
export const PYODIDE_VERSION = "314.0.3";
export const MAX_SOURCE_BYTES = 64 * 1024;
export const MAX_INPUT_BYTES = 1024 * 1024;
export const MAX_OUTPUT_BYTES = 1024 * 1024;
export const MAX_LOG_BYTES = 32 * 1024;
export const MAX_RESULT_ITEMS = 100_000;
export const MAX_RESULT_DEPTH = 32;
export const ALLOWED_PACKAGES = Object.freeze([
  "numpy",
  "pandas",
  "scipy",
  "statsmodels",
  "scikit-learn",
]);
export const BLOCKED_JS_GLOBALS = Object.freeze([
  "Deno",
  "process",
  "Bun",
  "Buffer",
  "require",
  "module",
  "exports",
  "fetch",
  "WebSocket",
  "Worker",
  "SharedWorker",
  "ServiceWorker",
  "importScripts",
  "XMLHttpRequest",
  "WebTransport",
  "navigator",
  "location",
  "document",
  "indexedDB",
  "caches",
  "localStorage",
  "sessionStorage",
  "open",
]);

const encoder = new TextEncoder();

export function byteLength(value) {
  return encoder.encode(value).byteLength;
}

function requireObject(value, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
}

function requireProtocol(value) {
  if (value.protocol !== PROTOCOL) throw new Error("compute protocol is unsupported");
}

function parsePackages(packages) {
  const selected = packages ?? [];
  if (
    !Array.isArray(selected) ||
    selected.length > ALLOWED_PACKAGES.length ||
    new Set(selected).size !== selected.length ||
    selected.some((name) => !ALLOWED_PACKAGES.includes(name))
  ) {
    throw new Error("compute package selection is invalid");
  }
  return selected;
}

function parseSeed(seed) {
  const value = seed ?? 0;
  if (!Number.isSafeInteger(value) || value < 0 || value > 0xffffffff) {
    throw new Error("compute seed is invalid");
  }
  return value;
}

function parseId(id) {
  if (typeof id !== "string" || !/^[0-9a-f]{32}$/.test(id)) {
    throw new Error("compute request id is invalid");
  }
  return id;
}

function parseSource(source) {
  if (
    typeof source !== "string" ||
    source.trim() === "" ||
    byteLength(source) > MAX_SOURCE_BYTES ||
    source.includes("\0")
  ) {
    throw new Error("compute source is invalid");
  }
  return source;
}

function parseInputs(inputs) {
  const value = inputs ?? {};
  let encodedInputs;
  try {
    encodedInputs = JSON.stringify(value);
  } catch {
    throw new Error("compute inputs must be JSON");
  }
  if (encodedInputs === undefined || byteLength(encodedInputs) > MAX_INPUT_BYTES) {
    throw new Error("compute inputs exceed the size limit");
  }
  return { inputs: value, encodedInputs };
}

function parseLanguage(language) {
  if (language !== "python" && language !== "javascript") {
    throw new Error("compute language is unsupported");
  }
  return language;
}

export function validateInit(value) {
  requireObject(value, "compute init");
  const allowedKeys = new Set(["protocol", "type", "language", "packages"]);
  if (Object.keys(value).some((key) => !allowedKeys.has(key))) {
    throw new Error("compute init contains unsupported fields");
  }
  requireProtocol(value);
  if (value.type !== "init") throw new Error("compute init type is invalid");
  const language = parseLanguage(value.language);
  if (language === "javascript") {
    if (value.packages !== undefined) {
      throw new Error("javascript compute does not accept packages");
    }
    return { protocol: PROTOCOL, type: "init", language };
  }
  return {
    protocol: PROTOCOL,
    type: "init",
    language,
    packages: parsePackages(value.packages),
  };
}

export function validateRun(value) {
  requireObject(value, "compute run");
  const allowedKeys = new Set(["protocol", "type", "id", "source", "inputs", "seed"]);
  if (Object.keys(value).some((key) => !allowedKeys.has(key))) {
    throw new Error("compute run contains unsupported fields");
  }
  requireProtocol(value);
  if (value.type !== "run") throw new Error("compute run type is invalid");
  const { inputs, encodedInputs } = parseInputs(value.inputs);
  return {
    protocol: PROTOCOL,
    type: "run",
    id: parseId(value.id),
    source: parseSource(value.source),
    inputs,
    encodedInputs,
    seed: parseSeed(value.seed),
  };
}

export function validateShutdown(value) {
  requireObject(value, "compute shutdown");
  const allowedKeys = new Set(["protocol", "type"]);
  if (Object.keys(value).some((key) => !allowedKeys.has(key))) {
    throw new Error("compute shutdown contains unsupported fields");
  }
  requireProtocol(value);
  if (value.type !== "shutdown") throw new Error("compute shutdown type is invalid");
  return { protocol: PROTOCOL, type: "shutdown" };
}

export function parseHostMessage(value) {
  requireObject(value, "compute host message");
  if (value.type === "init") return validateInit(value);
  if (value.type === "run") return validateRun(value);
  if (value.type === "shutdown") return validateShutdown(value);
  throw new Error("compute host message type is unsupported");
}

export function boundedLogger(limit = MAX_LOG_BYTES) {
  const entries = [];
  let bytes = 0;
  let truncated = false;
  return {
    push(stream, message) {
      if (truncated) return;
      const text = String(message);
      const next = byteLength(text);
      if (bytes + next > limit) {
        truncated = true;
        entries.push({ stream: "system", text: "[compute log truncated]" });
        return;
      }
      bytes += next;
      entries.push({ stream, text });
    },
    entries() {
      return entries.slice();
    },
  };
}

function claimItems(state, count = 1) {
  state.items += count;
  if (state.items > MAX_RESULT_ITEMS) {
    throw new Error("compute result contains too many items");
  }
}

export function normalizeResult(value, depth = 0, state = { items: 0 }) {
  if (depth > MAX_RESULT_DEPTH) {
    throw new Error("compute result is nested too deeply");
  }
  if (value === null || typeof value === "string" || typeof value === "boolean") {
    claimItems(state);
    return value;
  }
  if (typeof value === "number") {
    claimItems(state);
    return Number.isFinite(value) ? value : null;
  }
  if (typeof value === "bigint") {
    throw new TypeError("compute result type is unsupported: bigint");
  }
  if (typeof value === "undefined" || typeof value === "function" || typeof value === "symbol") {
    throw new TypeError(`compute result type is unsupported: ${typeof value}`);
  }
  if (value instanceof Date) {
    claimItems(state);
    return value.toISOString();
  }
  if (ArrayBuffer.isView(value)) {
    const items = Array.from(value);
    claimItems(state, items.length);
    return items.map((item) => normalizeResult(item, depth + 1, state));
  }
  if (value instanceof ArrayBuffer) {
    const items = Array.from(new Uint8Array(value));
    claimItems(state, items.length);
    return items.map((item) => normalizeResult(item, depth + 1, state));
  }
  if (Array.isArray(value)) {
    claimItems(state, value.length);
    return value.map((item) => normalizeResult(item, depth + 1, state));
  }
  if (typeof value === "object") {
    const prototype = Object.getPrototypeOf(value);
    if (prototype !== Object.prototype && prototype !== null) {
      throw new TypeError(
        `compute result type is unsupported: ${value.constructor?.name ?? "object"}`,
      );
    }
    const keys = Object.keys(value);
    claimItems(state, keys.length);
    const result = {};
    for (const key of keys) {
      result[key] = normalizeResult(value[key], depth + 1, state);
    }
    return result;
  }
  throw new TypeError(`compute result type is unsupported: ${typeof value}`);
}

export function seedMathRandom(seed) {
  let a = seed >>> 0;
  return function random() {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
