import net from "node:net";

const PROTOCOL = "guruterminal-tool/1";
// Keep this equal to the Rust broker's per-frame ceiling. Counting resets after
// each newline-delimited response.
const MAX_RESPONSE_FRAME_BYTES = 5 * 1024 * 1024;

let indeterminateDeliveryHandler = (error) => {
  process.stderr.write(`Guru Terminal broker delivery became indeterminate: ${error.message}\n`);
  const exitCode =
    error.message === "Tool broker closed before committing the response"
      ? 71
      : error.message === "Tool broker returned a malformed commit barrier"
        ? 72
        : error.message === "Tool broker connection failed"
          ? 73
          : error.message === "Tool broker closed without a response"
            ? 74
            : error.message === "Tool broker returned more than one response"
              ? 75
              : 70;
  process.exit(exitCode);
};

// A lost commit barrier after the client ACK cannot be represented as an
// ordinary Tool failure: Rust may already have committed the turn-local
// result. Production therefore terminates the agent run so its entire
// ToolCapture is discarded. Tests replace the non-returning exit handler.
export function setIndeterminateDeliveryHandlerForTests(handler) {
  const previous = indeterminateDeliveryHandler;
  indeterminateDeliveryHandler = handler;
  return () => {
    indeterminateDeliveryHandler = previous;
  };
}

function brokerConfig() {
  const socketPath = process.env.GURUTERMINAL_BROKER_SOCKET;
  const token = process.env.GURUTERMINAL_BROKER_TOKEN;
  if (!socketPath || !token) {
    throw new Error("Guru Terminal tool broker is unavailable");
  }
  return { socketPath, token };
}

function hasExactKeys(value, expected) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const keys = Object.keys(value);
  return (
    keys.length === expected.length &&
    expected.every((key) => Object.hasOwn(value, key))
  );
}

export function requestBroker(method, params, signal) {
  if (signal?.aborted) {
    return Promise.reject(new Error("Tool call aborted"));
  }
  const { socketPath, token } = brokerConfig();
  const id = crypto.randomUUID();

  return new Promise((resolve, reject) => {
    let settled = false;
    let received = "";
    let terminal;
    let deliveryAcknowledged = false;
    const socket = net.createConnection({ path: socketPath, allowHalfOpen: true });

    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      signal?.removeEventListener("abort", onAbort);
      socket.destroy();
      if (error) reject(error);
      else resolve(value);
    };

    const fail = (error) => {
      if (deliveryAcknowledged) indeterminateDeliveryHandler(error);
      finish(error);
    };

    const onAbort = () => {
      if (!deliveryAcknowledged) finish(new Error("Tool call aborted"));
    };
    signal?.addEventListener("abort", onAbort, { once: true });
    if (signal?.aborted) {
      onAbort();
      return;
    }
    socket.setEncoding("utf8");
    socket.on("connect", () => {
      const request = { protocol: PROTOCOL, id, token, method, params };
      socket.write(`${JSON.stringify(request)}\n`);
    });
    socket.on("data", (chunk) => {
      received += chunk;
      while (!settled) {
        const newline = received.indexOf("\n");
        if (newline < 0) {
          if (Buffer.byteLength(received, "utf8") > MAX_RESPONSE_FRAME_BYTES) {
            fail(new Error("Tool broker response exceeded the size limit"));
          }
          return;
        }
        const frame = received.slice(0, newline);
        received = received.slice(newline + 1);
        if (Buffer.byteLength(frame, "utf8") > MAX_RESPONSE_FRAME_BYTES) {
          fail(new Error("Tool broker response exceeded the size limit"));
          return;
        }

        let parsed;
        try {
          parsed = JSON.parse(frame);
        } catch {
          fail(new Error("Tool broker returned malformed JSON"));
          return;
        }

        if (deliveryAcknowledged) {
          if (
            !hasExactKeys(parsed, ["protocol", "id", "committed"]) ||
            parsed.protocol !== PROTOCOL ||
            parsed.id !== id ||
            parsed.committed !== true
          ) {
            fail(new Error("Tool broker returned a malformed commit barrier"));
          } else {
            finish(terminal.error, terminal.value);
          }
          return;
        }

        const isError = hasExactKeys(parsed, ["protocol", "id", "ok", "error"]);
        const isResult = hasExactKeys(parsed, ["protocol", "id", "ok", "result"]);
        if (!isError && !isResult) {
          fail(new Error("Tool broker returned a malformed response"));
        } else if (parsed.protocol !== PROTOCOL || parsed.id !== id) {
          fail(new Error("Tool broker response identity mismatch"));
        } else if (isError) {
          if (parsed.ok !== false) {
            fail(new Error("Tool broker returned a malformed response"));
          } else {
            terminal = {
              error: new Error(parsed.error?.message ?? "Tool broker rejected the call"),
            };
          }
        } else if (parsed.ok !== true) {
          fail(new Error("Tool broker returned a malformed response"));
        } else {
          terminal = { value: parsed.result };
        }
        if (!settled && terminal) {
          if (received.length > 0) {
            fail(new Error("Tool broker returned more than one response"));
            return;
          }
          deliveryAcknowledged = true;
          signal?.removeEventListener("abort", onAbort);
          // Write the ACK without half-closing. `socket.end(ack)` can deliver
          // FIN before the ACK line on Unix sockets, so Rust never reaches
          // the commit barrier and the client exits 71.
          socket.write(`${JSON.stringify({
            protocol: PROTOCOL,
            id,
            delivered: true,
          })}\n`);
          return;
        }
      }
    });
    socket.on("error", () => fail(new Error("Tool broker connection failed")));
    socket.on("end", () => {
      if (settled) return;
      if (deliveryAcknowledged && received.includes("\n")) {
        const newline = received.indexOf("\n");
        const frame = received.slice(0, newline);
        received = received.slice(newline + 1);
        let parsed;
        try {
          parsed = JSON.parse(frame);
        } catch {
          fail(new Error("Tool broker returned a malformed commit barrier"));
          return;
        }
        if (
          hasExactKeys(parsed, ["protocol", "id", "committed"]) &&
          parsed.protocol === PROTOCOL &&
          parsed.id === id &&
          parsed.committed === true
        ) {
          finish(terminal.error, terminal.value);
          return;
        }
        fail(new Error("Tool broker returned a malformed commit barrier"));
        return;
      }
      fail(new Error("Tool broker closed before committing the response"));
    });
    socket.on("close", () => {
      if (!settled) fail(new Error("Tool broker closed without a response"));
    });
  });
}
