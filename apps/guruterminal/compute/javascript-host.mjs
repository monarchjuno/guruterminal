import {
  BLOCKED_JS_GLOBALS,
  MAX_OUTPUT_BYTES,
  PROTOCOL,
  boundedLogger,
  byteLength,
  normalizeResult,
  parseHostMessage,
} from "./contract.mjs";

const CELL_TIMEOUT_MS = 28_000;

export const JAVASCRIPT_CELL_SOURCE = `
"use strict";
const BLOCKED = ${JSON.stringify(BLOCKED_JS_GLOBALS)};
const MAX_RESULT_ITEMS = 100000;
const MAX_RESULT_DEPTH = 32;

function claimItems(state, count) {
  state.items += count;
  if (state.items > MAX_RESULT_ITEMS) {
    throw new Error("compute result contains too many items");
  }
}

function normalizeResult(value, depth, state) {
  depth = depth || 0;
  state = state || { items: 0 };
  if (depth > MAX_RESULT_DEPTH) {
    throw new Error("compute result is nested too deeply");
  }
  if (value === null || typeof value === "string" || typeof value === "boolean") {
    claimItems(state, 1);
    return value;
  }
  if (typeof value === "number") {
    claimItems(state, 1);
    return Number.isFinite(value) ? value : null;
  }
  if (typeof value === "bigint") {
    throw new TypeError("compute result type is unsupported: bigint");
  }
  if (typeof value === "undefined" || typeof value === "function" || typeof value === "symbol") {
    throw new TypeError("compute result type is unsupported: " + typeof value);
  }
  if (value instanceof Date) {
    claimItems(state, 1);
    return value.toISOString();
  }
  if (ArrayBuffer.isView(value)) {
    const items = Array.from(value);
    claimItems(state, items.length);
    return items.map(function (item) { return normalizeResult(item, depth + 1, state); });
  }
  if (typeof ArrayBuffer !== "undefined" && value instanceof ArrayBuffer) {
    const items = Array.from(new Uint8Array(value));
    claimItems(state, items.length);
    return items.map(function (item) { return normalizeResult(item, depth + 1, state); });
  }
  if (Array.isArray(value)) {
    claimItems(state, value.length);
    return value.map(function (item) { return normalizeResult(item, depth + 1, state); });
  }
  if (typeof value === "object") {
    const prototype = Object.getPrototypeOf(value);
    if (prototype !== Object.prototype && prototype !== null) {
      throw new TypeError("compute result type is unsupported: " + ((value.constructor && value.constructor.name) || "object"));
    }
    const keys = Object.keys(value);
    claimItems(state, keys.length);
    const result = {};
    for (let i = 0; i < keys.length; i++) {
      result[keys[i]] = normalizeResult(value[keys[i]], depth + 1, state);
    }
    return result;
  }
  throw new TypeError("compute result type is unsupported: " + typeof value);
}

function seedMathRandom(seed) {
  let a = seed >>> 0;
  return function random() {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function neutralize() {
  for (let i = 0; i < BLOCKED.length; i++) {
    try {
      Object.defineProperty(globalThis, BLOCKED[i], {
        value: undefined,
        writable: false,
        enumerable: false,
        configurable: false,
      });
    } catch (_error) {
      try { globalThis[BLOCKED[i]] = undefined; } catch (_ignored) {}
    }
  }
}

function installConsole(logs) {
  function write(stream) {
    return function () {
      const text = Array.prototype.map.call(arguments, function (value) {
        return typeof value === "string" ? value : String(value);
      }).join(" ");
      logs.push({ stream: stream, text: text });
    };
  }
  const consoleObject = {
    log: write("stdout"),
    info: write("stdout"),
    debug: write("stdout"),
    warn: write("stderr"),
    error: write("stderr"),
  };
  try { globalThis.console = consoleObject; } catch (_error) {}
  return consoleObject;
}

neutralize();
Math.random = seedMathRandom(0);

self.onmessage = function (event) {
  const logs = [];
  const bounded = [];
  let logBytes = 0;
  function pushLog(entry) {
    const text = String(entry.text);
    const size = new TextEncoder().encode(text).byteLength;
    if (logBytes + size > 32768) {
      if (bounded.length === 0 || bounded[bounded.length - 1].text !== "[compute log truncated]") {
        bounded.push({ stream: "system", text: "[compute log truncated]" });
      }
      return;
    }
    logBytes += size;
    bounded.push({ stream: entry.stream, text: text });
  }
  installConsole(logs);
  Promise.resolve()
    .then(function () {
      const source = event.data.source;
      const inputs = event.data.inputs;
      const seed = event.data.seed >>> 0;
      Math.random = seedMathRandom(seed);
      neutralize();
      const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
      const runner = new AsyncFunction(
        "inputs",
        '"use strict";\\n' + source + '\\n;if (typeof main !== "function") throw new TypeError("compute source must define callable main(inputs)");\\nreturn await main(inputs);',
      );
      return runner(inputs);
    })
    .then(function (result) {
      for (let i = 0; i < logs.length; i++) pushLog(logs[i]);
      self.postMessage({ ok: true, result: normalizeResult(result), logs: bounded });
    })
    .catch(function (error) {
      for (let i = 0; i < logs.length; i++) pushLog(logs[i]);
      self.postMessage({
        ok: false,
        error: String(error && error.message ? error.message : error),
        logs: bounded,
      });
    });
};
`;

async function* stdinLines() {
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let pending = "";
  let pendingBytes = 0;
  for await (const chunk of Deno.stdin.readable) {
    pendingBytes += chunk.byteLength;
    if (pendingBytes > 2 * 1024 * 1024) {
      throw new Error("compute request exceeded the frame limit");
    }
    pending += decoder.decode(chunk, { stream: true });
    let newline;
    while ((newline = pending.indexOf("\n")) >= 0) {
      const line = pending.slice(0, newline).trim();
      pending = pending.slice(newline + 1);
      pendingBytes = byteLength(pending);
      if (line) yield line;
    }
  }
  pending += decoder.decode();
  const tail = pending.trim();
  if (tail) yield tail;
}

async function respond(value) {
  let encoded = `${JSON.stringify(value)}\n`;
  if (byteLength(encoded) > MAX_OUTPUT_BYTES) {
    encoded = `${JSON.stringify({
      protocol: PROTOCOL,
      type: value.type ?? "result",
      id: value.id ?? null,
      ok: false,
      error: { code: "output_too_large", message: "compute output exceeded the size limit" },
    })}\n`;
  }
  await Deno.stdout.write(new TextEncoder().encode(encoded));
}

function sanitizeError(error) {
  return String(error?.message ?? error ?? "compute execution failed").slice(0, 16 * 1024);
}

function runCell(message) {
  const blob = new Blob([JAVASCRIPT_CELL_SOURCE], { type: "text/javascript" });
  const url = URL.createObjectURL(blob);
  const worker = new Worker(url, {
    type: "module",
    deno: {
      namespace: false,
      permissions: "none",
    },
  });
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      worker.terminate();
      URL.revokeObjectURL(url);
      reject(new Error("compute execution timed out"));
    }, CELL_TIMEOUT_MS);
    const finish = (fn) => (value) => {
      clearTimeout(timer);
      worker.terminate();
      URL.revokeObjectURL(url);
      fn(value);
    };
    worker.onmessage = finish((event) => resolve(event.data));
    worker.onerror = finish((event) => {
      reject(new Error(event?.message || "compute worker failed"));
    });
    worker.onmessageerror = finish(() => {
      reject(new Error("compute worker message is invalid"));
    });
    worker.postMessage({
      source: message.source,
      inputs: message.inputs,
      seed: message.seed,
    });
  });
}

async function main() {
  const lines = stdinLines();
  const first = await lines.next();
  if (first.done) throw new Error("compute init is missing");
  const init = parseHostMessage(JSON.parse(first.value));
  if (init.type !== "init" || init.language !== "javascript") {
    throw new Error("javascript compute host received an invalid init");
  }
  await respond({ protocol: PROTOCOL, type: "ready", language: "javascript" });

  for await (const line of lines) {
    const message = parseHostMessage(JSON.parse(line));
    if (message.type === "shutdown") {
      await respond({ protocol: PROTOCOL, type: "bye" });
      return;
    }
    if (message.type !== "run") {
      throw new Error("javascript compute host received an invalid message");
    }
    try {
      const payload = await runCell(message);
      if (!payload || payload.ok !== true) {
        await respond({
          protocol: PROTOCOL,
          type: "result",
          id: message.id,
          ok: false,
          error: {
            code: "compute_failed",
            message: sanitizeError(payload?.error || "compute execution failed"),
          },
          logs: Array.isArray(payload?.logs) ? payload.logs : boundedLogger().entries(),
        });
        continue;
      }
      await respond({
        protocol: PROTOCOL,
        type: "result",
        id: message.id,
        ok: true,
        result: normalizeResult(payload.result),
        logs: payload.logs ?? [],
        runtime: {
          language: "javascript",
          deno: Deno.version.deno,
          v8: Deno.version.v8,
        },
      });
    } catch (error) {
      const timedOut = String(error?.message ?? "").includes("timed out");
      await respond({
        protocol: PROTOCOL,
        type: "result",
        id: message.id,
        ok: false,
        error: {
          code: timedOut ? "timeout" : "compute_failed",
          message: sanitizeError(error),
        },
      });
    }
  }
}

try {
  await main();
} catch (error) {
  await respond({
    protocol: PROTOCOL,
    type: "result",
    id: null,
    ok: false,
    error: { code: "compute_failed", message: sanitizeError(error) },
  });
  Deno.exit(1);
}
