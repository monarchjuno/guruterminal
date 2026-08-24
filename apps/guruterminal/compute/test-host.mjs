import { createInterface } from "node:readline";
import process from "node:process";

const PROTOCOL = "guruterminal-compute/2";
const PACKAGE_VERSIONS = {
  numpy: "2.4.3",
  pandas: "3.0.2",
  scipy: "1.18.0",
  statsmodels: "0.14.6",
  "scikit-learn": "1.8.0",
};

function write(value) {
  process.stdout.write(`${JSON.stringify(value)}\n`);
}

function runtimeFor(init) {
  if (init.language === "javascript") {
    return { language: "javascript", deno: "2.9.5", v8: "fake" };
  }
  const packages = {};
  for (const name of init.packages ?? []) {
    packages[name] = PACKAGE_VERSIONS[name];
  }
  return {
    language: "python",
    deno: "2.9.5",
    v8: "fake",
    pyodide: "314.0.3",
    python: "3.14.0",
    packages,
  };
}

const pid = process.pid;
let init = null;
let calls = 0;

const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
for await (const line of lines) {
  const trimmed = line.trim();
  if (!trimmed) continue;
  const message = JSON.parse(trimmed);
  if (message.type === "init") {
    init = message;
    write({ protocol: PROTOCOL, type: "ready", language: message.language });
    continue;
  }
  if (message.type === "shutdown") {
    write({ protocol: PROTOCOL, type: "bye" });
    process.exit(0);
  }
  if (message.type !== "run" || init == null) {
    process.exit(3);
  }
  if (String(message.source).includes("__crash__")) {
    process.exit(2);
  }
  // Hang until the Rust I/O deadline so the retained host is poisoned.
  if (String(message.source).includes("__timeout__")) {
    await new Promise(() => {});
  }
  // Completed in-host timeout frame, matching javascript-host.mjs.
  if (String(message.source).includes("__cell_timeout__")) {
    calls += 1;
    write({
      protocol: PROTOCOL,
      type: "result",
      id: message.id,
      ok: false,
      error: { code: "timeout", message: "compute execution timed out" },
    });
    continue;
  }
  if (String(message.source).includes("__fail__")) {
    calls += 1;
    write({
      protocol: PROTOCOL,
      type: "result",
      id: message.id,
      ok: false,
      error: { code: "compute_failed", message: "cell failed" },
    });
    continue;
  }
  calls += 1;
  write({
    protocol: PROTOCOL,
    type: "result",
    id: message.id,
    ok: true,
    result: {
      pid,
      calls,
      seed: message.seed ?? 0,
      language: init.language,
      packages: init.packages ?? [],
      inputs: message.inputs ?? {},
    },
    logs: [],
    runtime: runtimeFor(init),
  });
}
