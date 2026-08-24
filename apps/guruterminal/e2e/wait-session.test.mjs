import assert from "node:assert/strict";
import { createServer } from "node:http";
import { createServer as createNetServer } from "node:net";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";
import {
  classifyWebdriverPort,
  listenerOwnedBy,
  resolveWebdriverPort,
  sessionIsLive,
  webdriverIsReady,
  webdriverStatus,
  writeSession,
} from "./wait-session-lib.mjs";

const helper = join(dirname(fileURLToPath(import.meta.url)), "wait-session.mjs");

function listenHttp(handler) {
  const server = createServer(handler);
  server.unref();
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => {
      resolve({
        port: server.address().port,
        close: () => {
          if (typeof server.closeAllConnections === "function") {
            server.closeAllConnections();
          }
          server.close();
        },
      });
    });
  });
}

function listenTcp() {
  const server = createNetServer((socket) => {
    socket.end();
  });
  server.unref();
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => {
      resolve({
        port: server.address().port,
        close: () => {
          if (typeof server.closeAllConnections === "function") {
            server.closeAllConnections();
          }
          server.close();
        },
      });
    });
  });
}

function statusHandler(ready) {
  return (request, response) => {
    if (request.url !== "/status") {
      response.writeHead(404);
      response.end();
      return;
    }
    response.writeHead(200, { "content-type": "application/json" });
    response.end(
      JSON.stringify({
        value: {
          ready,
          message: ready
            ? "tauri-plugin-webdriver is ready"
            : "waiting for webview initialization",
        },
      }),
    );
  };
}

function runWait(args) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [helper, ...args], {
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stderr = "";
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("exit", (code) => {
      resolve({ code, stderr });
    });
  });
}

const tcp = await listenTcp();
const directory = mkdtempSync(join(tmpdir(), "guruterminal-wait-"));
try {
  const sessionPath = join(directory, "current-session.json");
  writeSession(sessionPath, process.pid, tcp.port, "development");
  assert.equal(await sessionIsLive(sessionPath), false);
  assert.equal(await webdriverIsReady(tcp.port), false);
  assert.equal(await classifyWebdriverPort(tcp.port), "foreign");
  const resolved = await resolveWebdriverPort(tcp.port);
  assert.notEqual(resolved, tcp.port);
  assert.equal(await classifyWebdriverPort(resolved), "free");
} finally {
  tcp.close();
  rmSync(directory, { recursive: true, force: true });
}

const notReady = await listenHttp(statusHandler(false));
try {
  assert.equal((await webdriverStatus(notReady.port))?.value?.ready, false);
  assert.equal(await webdriverIsReady(notReady.port), false);
} finally {
  notReady.close();
}

const ready = await listenHttp(statusHandler(true));
const readyDirectory = mkdtempSync(join(tmpdir(), "guruterminal-wait-"));
try {
  const sessionPath = join(readyDirectory, "current-session.json");
  writeSession(sessionPath, process.pid, ready.port, "development");
  assert.equal(await webdriverIsReady(ready.port), true);
  assert.equal(await sessionIsLive(sessionPath), true);
  assert.equal(listenerOwnedBy(ready.port, process.pid), true);

  const stranger = spawn(process.execPath, ["-e", "setTimeout(() => {}, 30_000)"], {
    stdio: "ignore",
  });
  try {
    const ignored = await runWait([
      "--wait-owned",
      "--pid",
      String(stranger.pid),
      "--port",
      String(ready.port),
      "--timeout-ms",
      "800",
    ]);
    assert.equal(ignored.code, 1);
    assert.match(ignored.stderr, /did not become ready/);
  } finally {
    stranger.kill("SIGTERM");
  }

  const owned = await runWait([
    "--wait-owned",
    "--pid",
    String(process.pid),
    "--port",
    String(ready.port),
    "--timeout-ms",
    "2000",
  ]);
  assert.equal(owned.code, 0, owned.stderr);
} finally {
  ready.close();
  rmSync(readyDirectory, { recursive: true, force: true });
}

console.log("wait-session readiness contract passed");
