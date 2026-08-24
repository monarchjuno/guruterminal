import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  MAX_OUTPUT_BYTES,
  PROTOCOL,
  PYODIDE_VERSION,
  boundedLogger,
  byteLength,
  parseHostMessage,
} from "./contract.mjs";

const runtimeRoot = dirname(fileURLToPath(import.meta.url));
const pyodideRoot = join(runtimeRoot, "pyodide");

const PYTHON_DRIVER = String.raw`
def __guru_cell():
    import datetime as _datetime
    import decimal as _decimal
    import json as _json
    import math as _math
    import random as _random
    import sys as _sys

    _MAX_ITEMS = 100_000
    _seen_items = 0

    def _claim(count=1):
        nonlocal _seen_items
        _seen_items += count
        if _seen_items > _MAX_ITEMS:
            raise ValueError("compute result contains too many items")

    def _normalize(value, depth=0):
        if depth > 32:
            raise ValueError("compute result is nested too deeply")
        if value is None or isinstance(value, (bool, str, int)):
            _claim()
            return value
        if isinstance(value, float):
            _claim()
            return value if _math.isfinite(value) else None
        if isinstance(value, _decimal.Decimal):
            _claim()
            return str(value)
        if isinstance(value, (_datetime.datetime, _datetime.date, _datetime.time)):
            _claim()
            return value.isoformat()

        try:
            import numpy as _np
            if isinstance(value, _np.ndarray):
                _claim(int(value.size))
                return _normalize(value.tolist(), depth + 1)
            if isinstance(value, _np.generic):
                return _normalize(value.item(), depth + 1)
        except ImportError:
            pass

        try:
            import pandas as _pd
            if isinstance(value, _pd.DataFrame):
                _claim(int(value.shape[0] * max(value.shape[1], 1)))
                return {
                    "kind": "table",
                    "columns": [_normalize(item, depth + 1) for item in value.columns.tolist()],
                    "index": [_normalize(item, depth + 1) for item in value.index.tolist()],
                    "rows": _normalize(value.to_numpy().tolist(), depth + 1),
                }
            if isinstance(value, _pd.Series):
                _claim(int(value.size))
                return {
                    "kind": "series",
                    "name": _normalize(value.name, depth + 1),
                    "index": [_normalize(item, depth + 1) for item in value.index.tolist()],
                    "values": _normalize(value.tolist(), depth + 1),
                }
            if isinstance(value, (_pd.Timestamp, _pd.Timedelta)):
                return str(value)
        except ImportError:
            pass

        if isinstance(value, dict):
            _claim(len(value))
            result = {}
            for key, item in value.items():
                if not isinstance(key, str):
                    raise TypeError("compute result object keys must be strings")
                result[key] = _normalize(item, depth + 1)
            return result
        if isinstance(value, (list, tuple)):
            _claim(len(value))
            return [_normalize(item, depth + 1) for item in value]
        if isinstance(value, (set, frozenset)):
            _claim(len(value))
            return [_normalize(item, depth + 1) for item in sorted(value, key=repr)]
        raise TypeError(f"compute result type is unsupported: {type(value).__name__}")

    # Keep the loaded Pyodide process, but make imports cell-local. A shallow
    # module-namespace snapshot is enough to undo ordinary attribute mutation
    # while preserving the expensive runtime/package initialization itself.
    _modules = _sys.modules
    _modules_before = dict(_modules)
    _module_namespaces = {}
    for _module_name, _module in _modules_before.items():
        try:
            _module_namespaces[_module_name] = dict(vars(_module))
        except (TypeError, AttributeError):
            pass

    try:
        _inputs = _json.loads(__guru_inputs_json)
        _random.seed(__guru_seed)
        try:
            import numpy as _np
            _np.random.seed(__guru_seed)
        except ImportError:
            pass

        _namespace = {"__name__": "__guruterminal_compute__"}
        exec(compile(__guru_source, "<compute>", "exec"), _namespace, _namespace)
        _main = _namespace.get("main")
        if not callable(_main):
            raise TypeError("compute source must define callable main(inputs)")
        _result = _main(_inputs)
        return _json.dumps(_normalize(_result), ensure_ascii=False, allow_nan=False, separators=(",", ":"))
    finally:
        _sys.modules = _modules
        for _module_name in tuple(_modules):
            if _module_name not in _modules_before:
                _modules.pop(_module_name, None)
        for _module_name, _module in _modules_before.items():
            _modules[_module_name] = _module
            _snapshot = _module_namespaces.get(_module_name)
            if _snapshot is None:
                continue
            try:
                _current = vars(_module)
                for _key in tuple(_current):
                    if _key not in _snapshot:
                        _current.pop(_key, None)
                _current.update(_snapshot)
            except (TypeError, AttributeError):
                pass

__guru_cell()
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
  return String(error?.message ?? error ?? "compute execution failed")
    .replaceAll(runtimeRoot, "<compute-runtime>")
    .slice(0, 16 * 1024);
}

function clearDriverGlobals(pyodide) {
  pyodide.runPython(
    "for _name in ('__guru_cell', '__guru_source', '__guru_inputs_json', '__guru_seed', '__guru_packages_json', '__guru_imports_json'):\n" +
      "    globals().pop(_name, None)\n",
  );
}

async function main() {
  const lines = stdinLines();
  const first = await lines.next();
  if (first.done) throw new Error("compute init is missing");
  const init = parseHostMessage(JSON.parse(first.value));
  if (init.type !== "init" || init.language !== "python") {
    throw new Error("python compute host received an invalid init");
  }

  const startupLogs = boundedLogger();
  const safeJsGlobals = Object.freeze(Object.create(null));
  const { loadPyodide } = await import("./pyodide/pyodide.mjs");
  const pyodide = await loadPyodide({
    indexURL: `${pyodideRoot}/`,
    jsglobals: safeJsGlobals,
    env: { HOME: "/home/pyodide", PYTHONHASHSEED: "0" },
    stdout: (message) => startupLogs.push("runtime", message),
    stderr: (message) => startupLogs.push("runtime", message),
  });
  await pyodide.loadPackage(init.packages, {
    messageCallback: (message) => startupLogs.push("runtime", message),
    errorCallback: (message) => startupLogs.push("runtime", message),
    checkIntegrity: true,
  });
  // A retained host restores each cell to this module baseline. Import the
  // requested native packages once before taking that baseline: unloading and
  // re-importing a Pyodide extension can leave its native runtime half-reset.
  const importNames = init.packages.map((name) => (name === "scikit-learn" ? "sklearn" : name));
  pyodide.globals.set("__guru_imports_json", JSON.stringify(importNames));
  pyodide.runPython(
    "import importlib as _importlib, json as _json; " +
      "[_importlib.import_module(name) for name in _json.loads(__guru_imports_json)]",
  );
  pyodide.globals.set("__guru_packages_json", JSON.stringify(init.packages));
  const versions = JSON.parse(
    pyodide.runPython(
      "import importlib.metadata as _metadata, json as _json; " +
        "_json.dumps({name: _metadata.version(name) for name in _json.loads(__guru_packages_json)})",
    ),
  );
  const pythonVersion = pyodide.runPython("import platform; platform.python_version()");
  clearDriverGlobals(pyodide);
  await respond({ protocol: PROTOCOL, type: "ready", language: "python" });

  for await (const line of lines) {
    const message = parseHostMessage(JSON.parse(line));
    if (message.type === "shutdown") {
      await respond({ protocol: PROTOCOL, type: "bye" });
      return;
    }
    if (message.type !== "run") {
      throw new Error("python compute host received an invalid message");
    }
    try {
      const logs = boundedLogger();
      pyodide.setStdout({ batched: (text) => logs.push("stdout", text) });
      pyodide.setStderr({ batched: (text) => logs.push("stderr", text) });
      pyodide.globals.set("__guru_source", message.source);
      pyodide.globals.set("__guru_inputs_json", message.encodedInputs);
      pyodide.globals.set("__guru_seed", message.seed);
      const resultJson = pyodide.runPython(PYTHON_DRIVER);
      clearDriverGlobals(pyodide);
      await respond({
        protocol: PROTOCOL,
        type: "result",
        id: message.id,
        ok: true,
        result: JSON.parse(resultJson),
        logs: logs.entries(),
        runtime: {
          language: "python",
          deno: Deno.version.deno,
          v8: Deno.version.v8,
          pyodide: PYODIDE_VERSION,
          python: pythonVersion,
          packages: versions,
        },
      });
    } catch (error) {
      clearDriverGlobals(pyodide);
      await respond({
        protocol: PROTOCOL,
        type: "result",
        id: message.id,
        ok: false,
        error: { code: "compute_failed", message: sanitizeError(error) },
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
