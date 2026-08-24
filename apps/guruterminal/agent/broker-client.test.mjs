import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import net from "node:net";
import { randomUUID } from "node:crypto";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  requestBroker,
  setIndeterminateDeliveryHandlerForTests,
} from "./broker-client.mjs";

async function setupBroker(t, respond) {
  const temporary = mkdtempSync(join(tmpdir(), "guruterminal-broker-protocol-"));
  const socketPath =
    process.platform === "win32"
      ? `\\\\.\\pipe\\guruterminal-broker-protocol-${randomUUID()}`
      : join(temporary, "broker.sock");
  const previousSocket = process.env.GURUTERMINAL_BROKER_SOCKET;
  const previousToken = process.env.GURUTERMINAL_BROKER_TOKEN;
  process.env.GURUTERMINAL_BROKER_SOCKET = socketPath;
  process.env.GURUTERMINAL_BROKER_TOKEN = "broker-test-token";

  const sockets = new Set();
  const server = net.createServer((socket) => {
    sockets.add(socket);
    socket.once("close", () => sockets.delete(socket));
    let buffered = "";
    socket.setEncoding("utf8");
    socket.on("data", (chunk) => {
      buffered += chunk;
      const newline = buffered.indexOf("\n");
      if (newline < 0) return;
      const request = JSON.parse(buffered.slice(0, newline));
      respond(socket, request);
    });
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(socketPath, resolve);
  });
  t.after(async () => {
    for (const socket of sockets) socket.destroy();
    await new Promise((resolve) => server.close(resolve));
    if (previousSocket === undefined) delete process.env.GURUTERMINAL_BROKER_SOCKET;
    else process.env.GURUTERMINAL_BROKER_SOCKET = previousSocket;
    if (previousToken === undefined) delete process.env.GURUTERMINAL_BROKER_TOKEN;
    else process.env.GURUTERMINAL_BROKER_TOKEN = previousToken;
    rmSync(temporary, { recursive: true, force: true });
  });
  return (signal = new AbortController().signal) =>
    requestBroker("guru.search", { query: "test" }, signal);
}

function sendTerminalAfterAck(socket, request, response, onAck = () => {}) {
  let buffered = "";
  const onData = (chunk) => {
    buffered += chunk;
    const newline = buffered.indexOf("\n");
    if (newline < 0) return;
    const ack = JSON.parse(buffered.slice(0, newline));
    assert.deepEqual(ack, {
      protocol: "guruterminal-tool/1",
      id: request.id,
      delivered: true,
    });
    socket.off("data", onData);
    onAck();
    socket.end(`${JSON.stringify({
      protocol: "guruterminal-tool/1",
      id: request.id,
      committed: true,
    })}\n`);
  };
  socket.removeAllListeners("data");
  socket.on("data", onData);
  socket.write(`${JSON.stringify(response)}\n`);
}

test("does not connect for an already-aborted call", async (t) => {
  let connected = false;
  const execute = await setupBroker(t, () => {
    connected = true;
  });
  const controller = new AbortController();
  controller.abort();
  await assert.rejects(execute(controller.signal), /Tool call aborted/);
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(connected, false);
});
test("settles a commit barrier that arrives after a delayed ACK write", async (t) => {
  const execute = await setupBroker(t, (socket, request) => {
    sendTerminalAfterAck(
      socket,
      request,
      {
        protocol: "guruterminal-tool/1",
        id: request.id,
        ok: true,
        result: { records: ["delayed-commit"] },
      },
      () => {
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 50);
      },
    );
  });
  assert.deepEqual(await execute(), { records: ["delayed-commit"] });
});

test("accepts one terminal result response", async (t) => {
  const execute = await setupBroker(t, (socket, request) => {
    sendTerminalAfterAck(socket, request, {
      protocol: "guruterminal-tool/1",
      id: request.id,
      ok: true,
      result: { records: ["lens-quality"] },
    });
  });
  assert.deepEqual(await execute(), { records: ["lens-quality"] });
});

test("rejects one terminal error response", async (t) => {
  const execute = await setupBroker(t, (socket, request) => {
    sendTerminalAfterAck(socket, request, {
      protocol: "guruterminal-tool/1",
      id: request.id,
      ok: false,
      error: { code: "method_denied", message: "broker denied the call" },
    });
  });
  await assert.rejects(execute(), /broker denied the call/);
});

test("rejects a delivered response when the commit barrier never arrives", async (t) => {
  const fatal = [];
  const restore = setIndeterminateDeliveryHandlerForTests((error) => fatal.push(error.message));
  t.after(restore);
  const execute = await setupBroker(t, (socket, request) => {
    let buffered = "";
    socket.removeAllListeners("data");
    socket.on("data", (chunk) => {
      buffered += chunk;
      if (buffered.includes("\n")) socket.end();
    });
    socket.write(`${JSON.stringify({
      protocol: "guruterminal-tool/1",
      id: request.id,
      ok: true,
      result: { records: [] },
    })}\n`);
  });
  await assert.rejects(execute(), /before committing/);
  assert.deepEqual(fatal, ["Tool broker closed before committing the response"]);
});

test("makes a malformed post-ACK commit barrier turn-fatal", async (t) => {
  const fatal = [];
  const restore = setIndeterminateDeliveryHandlerForTests((error) => fatal.push(error.message));
  t.after(restore);
  const execute = await setupBroker(t, (socket, request) => {
    socket.removeAllListeners("data");
    socket.once("data", () => {
      socket.end(`${JSON.stringify({
        protocol: "guruterminal-tool/1",
        id: request.id,
        committed: false,
      })}\n`);
    });
    socket.write(`${JSON.stringify({
      protocol: "guruterminal-tool/1",
      id: request.id,
      ok: true,
      result: { records: [] },
    })}\n`);
  });
  await assert.rejects(execute(), /malformed commit barrier/);
  assert.deepEqual(fatal, ["Tool broker returned a malformed commit barrier"]);
});

test("rejects legacy acknowledgement fields and malformed identities", async (t) => {
  await t.test("legacy phase", async (t) => {
    const execute = await setupBroker(t, (socket, request) => {
      socket.end(`${JSON.stringify({
        protocol: "guruterminal-tool/1",
        id: request.id,
        phase: "result",
        ok: true,
        result: {},
      })}\n`);
    });
    await assert.rejects(execute(), /malformed response/);
  });
  await t.test("wrong id", async (t) => {
    const execute = await setupBroker(t, (socket) => {
      socket.end(`${JSON.stringify({
        protocol: "guruterminal-tool/1",
        id: "wrong",
        ok: true,
        result: {},
      })}\n`);
    });
    await assert.rejects(execute(), /identity mismatch/);
  });
});

test("accepts a large bounded result and rejects an oversized frame", async (t) => {
  await t.test("bounded", async (t) => {
    const text = "x".repeat(2 * 1024 * 1024);
    const execute = await setupBroker(t, (socket, request) => {
      sendTerminalAfterAck(socket, request, {
        protocol: "guruterminal-tool/1",
        id: request.id,
        ok: true,
        result: { text },
      });
    });
    assert.equal((await execute()).text.length, text.length);
  });
  await t.test("oversized", async (t) => {
    const execute = await setupBroker(t, (socket, request) => {
      socket.end(`${JSON.stringify({
        protocol: "guruterminal-tool/1",
        id: request.id,
        ok: true,
        result: { text: "x".repeat(5 * 1024 * 1024) },
      })}\n`);
    });
    await assert.rejects(execute(), /exceeded the size limit/);
  });
});
