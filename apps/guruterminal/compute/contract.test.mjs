import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import {
  ALLOWED_PACKAGES,
  BLOCKED_JS_GLOBALS,
  MAX_LOG_BYTES,
  PROTOCOL,
  boundedLogger,
  normalizeResult,
  parseHostMessage,
  seedMathRandom,
  validateInit,
  validateRun,
} from "./contract.mjs";

const directory = dirname(fileURLToPath(import.meta.url));

function pythonInit(overrides = {}) {
  return {
    protocol: PROTOCOL,
    type: "init",
    language: "python",
    packages: ["numpy", "pandas"],
    ...overrides,
  };
}

function runMessage(overrides = {}) {
  return {
    protocol: PROTOCOL,
    type: "run",
    id: "a".repeat(32),
    source: "def main(inputs):\n    return inputs['value'] * 2",
    inputs: { value: 2 },
    seed: 7,
    ...overrides,
  };
}

test("protocol version is the retained NDJSON host contract", () => {
  assert.equal(PROTOCOL, "guruterminal-compute/2");
});

test("accepts python init with an exact package set and a bounded run", () => {
  const init = validateInit(pythonInit());
  assert.deepEqual(init.packages, ["numpy", "pandas"]);
  const parsed = validateRun(runMessage());
  assert.equal(parsed.seed, 7);
});

test("javascript init rejects packages and python rejects unbundled packages", () => {
  assert.throws(() => validateInit(pythonInit({ packages: ["requests"] })));
  assert.throws(() =>
    validateInit({
      protocol: PROTOCOL,
      type: "init",
      language: "javascript",
      packages: ["numpy"],
    }),
  );
  const js = validateInit({
    protocol: PROTOCOL,
    type: "init",
    language: "javascript",
  });
  assert.equal(js.language, "javascript");
  assert.deepEqual(ALLOWED_PACKAGES, [
    "numpy",
    "pandas",
    "scipy",
    "statsmodels",
    "scikit-learn",
  ]);
});

test("rejects mixed host messages and one-shot protocol leftovers", () => {
  assert.throws(() => parseHostMessage(runMessage({ protocol: "guruterminal-compute/1" })));
  assert.throws(() => parseHostMessage({ ...runMessage(), packages: ["numpy"] }));
  assert.throws(() => parseHostMessage({ ...pythonInit(), network: true }));
  assert.deepEqual(parseHostMessage({ protocol: PROTOCOL, type: "shutdown" }), {
    protocol: PROTOCOL,
    type: "shutdown",
  });
});

test("bounds logs without splitting the protocol response", () => {
  const logs = boundedLogger(MAX_LOG_BYTES);
  logs.push("stdout", "x".repeat(MAX_LOG_BYTES));
  logs.push("stderr", "overflow");
  assert.equal(logs.entries().at(-1).text, "[compute log truncated]");
});

test("javascript result normalization matches the bounded JSON contract", () => {
  assert.equal(normalizeResult(Number.POSITIVE_INFINITY), null);
  assert.deepEqual(normalizeResult(new Uint8Array([1, 2])), [1, 2]);
  assert.match(normalizeResult(new Date("2026-01-02T00:00:00.000Z")), /^2026-01-02T00:00:00.000Z$/);
  assert.throws(() => normalizeResult({ nested: { too: { deep: 1 } } }, 31));
  assert.throws(() => normalizeResult(undefined));
});

test("seeded Math.random is deterministic for a given seed", () => {
  const first = seedMathRandom(11);
  const second = seedMathRandom(11);
  assert.equal(first(), second());
  assert.notEqual(seedMathRandom(11)(), seedMathRandom(12)());
});

test("javascript host is a Pyodide-free permission-zero worker realm", () => {
  const host = readFileSync(join(directory, "javascript-host.mjs"), "utf8");
  const python = readFileSync(join(directory, "bootstrap.mjs"), "utf8");
  assert.match(host, /permissions:\s*"none"/);
  assert.match(host, /namespace:\s*false/);
  assert.match(host, /new Worker/);
  assert.match(host, /AsyncFunction/);
  assert.match(host, /JSON\.stringify\(BLOCKED_JS_GLOBALS\)/);
  assert.match(host, /Object\.defineProperty\(globalThis, BLOCKED\[i\]/);
  assert.doesNotMatch(host, /pyodide/i);
  assert.match(python, /loadPyodide/);
  assert.match(python, /_modules_before = dict\(_modules\)/);
  assert.match(python, /if _module_name not in _modules_before/);
  assert.match(python, /_current\.update\(_snapshot\)/);
  assert.match(python, /language !== "python"/);
  assert.match(host, /language !== "javascript"/);
  assert.deepEqual([...BLOCKED_JS_GLOBALS], [
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
});
