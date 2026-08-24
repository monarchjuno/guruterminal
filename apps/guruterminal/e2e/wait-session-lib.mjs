import { existsSync, writeFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import net from "node:net";

export const DEV_WEBDRIVER_PORT = 14440;
export const DEV_VITE_PORT = 1420;

const usage = `Usage:
  node wait-session.mjs --check <current-session.json>
  node wait-session.mjs --check-port <port>
  node wait-session.mjs --is-dev-session <current-session.json>
  node wait-session.mjs --recover <current-session.json>
  node wait-session.mjs --adopt-dev <current-session.json>
  node wait-session.mjs --wait-owned --pid <launcher-pid> --port <port> [--timeout-ms <ms>]
  node wait-session.mjs --resolve-port <preferred-port>
  node wait-session.mjs --write-session <current-session.json> --pid <pid> --port <port> --profile <development|e2e>
  node wait-session.mjs --pid <launcher-pid> --session <current-session.json> [--timeout-ms <ms>]`;

export function integerFlag(name, fallback, argv = process.argv) {
  const index = argv.indexOf(name);
  if (index < 0) return fallback;
  const value = Number(argv[index + 1]);
  if (!Number.isInteger(value) || value <= 0) {
    throw new Error(`${name} must be a positive integer\n${usage}`);
  }
  return value;
}

export function stringFlag(name, argv = process.argv) {
  const index = argv.indexOf(name);
  if (index < 0) return null;
  return argv[index + 1] ?? null;
}

export function pidIsRunning(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

export function portIsOpen(port, host = "127.0.0.1") {
  return new Promise((resolveOpen) => {
    const socket = net.createConnection({ host, port });
    socket.setTimeout(250);
    socket.once("connect", () => {
      socket.destroy();
      resolveOpen(true);
    });
    const closed = () => {
      socket.destroy();
      resolveOpen(false);
    };
    socket.once("error", closed);
    socket.once("timeout", closed);
  });
}

export async function webdriverStatus(port, host = "127.0.0.1") {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 400);
  try {
    const response = await fetch(`http://${host}:${port}/status`, {
      signal: controller.signal,
    });
    if (!response.ok) return null;
    const body = await response.json();
    if (!body || typeof body !== "object" || !("value" in body)) return null;
    return body;
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

export function webdriverValueIsReady(status) {
  return status?.value?.ready === true;
}

export async function webdriverIsReady(port, host = "127.0.0.1") {
  return webdriverValueIsReady(await webdriverStatus(port, host));
}

export async function readSession(sessionPath) {
  if (!existsSync(sessionPath)) return null;
  try {
    return JSON.parse(await readFile(sessionPath, "utf8"));
  } catch {
    return null;
  }
}

export function sessionLooksValid(session) {
  const port = session?.webdriverConfig?.port;
  return (
    (session?.profile === "development" || session?.profile === "e2e") &&
    session?.webdriverConfig?.protocol === "http" &&
    session?.webdriverConfig?.hostname === "127.0.0.1" &&
    session?.capabilities?.browserName === "tauri" &&
    Number.isInteger(port)
  );
}

export async function sessionIsLive(sessionPath) {
  const session = await readSession(sessionPath);
  if (!sessionLooksValid(session)) return false;
  return webdriverIsReady(
    session.webdriverConfig.port,
    session.webdriverConfig.hostname,
  );
}

export function writeSession(sessionPath, launcherPid, port, profile) {
  if (profile !== "development" && profile !== "e2e") {
    throw new Error("session profile must be development or e2e");
  }
  writeFileSync(
    sessionPath,
    `${JSON.stringify(
      {
        launcherPid,
        profile,
        webdriverConfig: {
          protocol: "http",
          hostname: "127.0.0.1",
          port,
          path: "/",
        },
        capabilities: { browserName: "tauri" },
      },
      null,
      2,
    )}\n`,
    { mode: 0o600 },
  );
}

export async function classifyWebdriverPort(port, host = "127.0.0.1") {
  if (await webdriverStatus(port, host)) return "webdriver";
  if (await portIsOpen(port, host)) return "foreign";
  return "free";
}

export function allocateFreePort() {
  return new Promise((resolvePort, reject) => {
    const server = net.createServer();
    server.unref();
    server.once("error", (error) => {
      reject(error);
    });
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      const port = address?.port;
      server.close((error) => {
        if (error) {
          reject(error);
          return;
        }
        if (!Number.isInteger(port) || port <= 0) {
          reject(new Error("failed to allocate a loopback WebDriver port"));
          return;
        }
        resolvePort(port);
      });
    });
  });
}

export async function resolveWebdriverPort(preferred) {
  if (!Number.isInteger(preferred) || preferred < 1024 || preferred > 65535) {
    throw new Error("TAURI_WEBDRIVER_PORT must be an integer from 1024 to 65535.");
  }
  const classification = await classifyWebdriverPort(preferred);
  if (classification !== "foreign") return preferred;
  const port = await allocateFreePort();
  console.error(
    `Guru Terminal WebDriver port ${preferred} is occupied by a non-WebDriver listener; using ${port}.`,
  );
  return port;
}

export function listeningPids(port) {
  try {
    const output = execFileSync(
      "lsof",
      ["-nP", `-iTCP:${port}`, "-sTCP:LISTEN", "-t"],
      { encoding: "utf8" },
    );
    return output
      .trim()
      .split(/\s+/)
      .map(Number)
      .filter((pid) => Number.isInteger(pid) && pid > 0);
  } catch {
    return [];
  }
}

export function listeningPid(port) {
  return listeningPids(port)[0] ?? null;
}

function childPids(pid) {
  try {
    return execFileSync("pgrep", ["-P", String(pid)], { encoding: "utf8" })
      .trim()
      .split(/\s+/)
      .map(Number)
      .filter((child) => Number.isInteger(child) && child > 0);
  } catch {
    return [];
  }
}

export function descendantPids(rootPid) {
  const seen = new Set();
  const stack = [rootPid];
  while (stack.length > 0) {
    const pid = stack.pop();
    if (seen.has(pid)) continue;
    seen.add(pid);
    stack.push(...childPids(pid));
  }
  return seen;
}

export function listenerOwnedBy(port, rootPid) {
  const tree = descendantPids(rootPid);
  return listeningPids(port).some((pid) => tree.has(pid));
}

async function waitUntil(predicate, { pid, timeoutMs, missingPidMessage, timeoutMessage }) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (!pidIsRunning(pid)) {
      throw new Error(missingPidMessage);
    }
    if (await predicate()) return;
    await new Promise((resolveWait) => setTimeout(resolveWait, 200));
  }
  throw new Error(timeoutMessage);
}

export async function main(argv = process.argv) {
  const checkSession = stringFlag("--check", argv);
  if (checkSession) {
    process.exit((await sessionIsLive(checkSession)) ? 0 : 1);
  }

  const isDevSession = stringFlag("--is-dev-session", argv);
  if (isDevSession) {
    const session = await readSession(isDevSession);
    process.exit(sessionLooksValid(session) && session.profile === "development" ? 0 : 1);
  }

  const checkPort = integerFlag("--check-port", 0, argv);
  if (checkPort) {
    const open =
      (await portIsOpen(checkPort, "127.0.0.1")) ||
      (await portIsOpen(checkPort, "::1"));
    process.exit(open ? 0 : 1);
  }

  const recoverPath = stringFlag("--recover", argv);
  if (recoverPath) {
    const session = await readSession(recoverPath);
    if (!sessionLooksValid(session) || session.profile !== "development") {
      process.exit(1);
    }
    if (await sessionIsLive(recoverPath)) process.exit(0);
    const launcherPid = session.launcherPid;
    if (!Number.isInteger(launcherPid) || launcherPid <= 0 || !pidIsRunning(launcherPid)) {
      process.exit(1);
    }
    await waitUntil(() => sessionIsLive(recoverPath), {
      pid: launcherPid,
      timeoutMs: integerFlag("--timeout-ms", 300_000, argv),
      missingPidMessage:
        "Guru Terminal development launcher exited before WebDriver returned",
      timeoutMessage:
        "Guru Terminal development WebDriver did not return within the timeout",
    });
    process.exit(0);
  }

  const adoptPath = stringFlag("--adopt-dev", argv);
  if (adoptPath) {
    const port = Number(process.env.TAURI_WEBDRIVER_PORT || DEV_WEBDRIVER_PORT);
    const viteUp =
      (await portIsOpen(DEV_VITE_PORT, "127.0.0.1")) ||
      (await portIsOpen(DEV_VITE_PORT, "::1"));
    if (!viteUp || !(await webdriverIsReady(port))) process.exit(1);
    const launcherPid = listeningPid(port);
    if (!launcherPid) process.exit(1);
    writeSession(adoptPath, launcherPid, port, "development");
    process.exit(0);
  }

  const resolvePreferred = integerFlag("--resolve-port", 0, argv);
  if (resolvePreferred) {
    process.stdout.write(String(await resolveWebdriverPort(resolvePreferred)));
    process.exit(0);
  }

  const writePath = stringFlag("--write-session", argv);
  if (writePath) {
    writeSession(
      writePath,
      integerFlag("--pid", 0, argv),
      integerFlag("--port", 0, argv),
      stringFlag("--profile", argv),
    );
    process.exit(0);
  }

  if (argv.includes("--wait-owned")) {
    const pid = integerFlag("--pid", 0, argv);
    const port = integerFlag("--port", 0, argv);
    const timeoutMs = integerFlag("--timeout-ms", 300_000, argv);
    if (!pid || !port) {
      throw new Error(usage);
    }
    await waitUntil(
      async () => (await webdriverIsReady(port)) && listenerOwnedBy(port, pid),
      {
        pid,
        timeoutMs,
        missingPidMessage:
          "Guru Terminal launcher exited before WebDriver became ready",
        timeoutMessage: `Guru Terminal WebDriver did not become ready within ${timeoutMs} milliseconds`,
      },
    );
    process.exit(0);
  }

  const pid = integerFlag("--pid", 0, argv);
  const sessionPath = stringFlag("--session", argv);
  const timeoutMs = integerFlag("--timeout-ms", 300_000, argv);
  if (!pid || !sessionPath) {
    throw new Error(usage);
  }

  await waitUntil(() => sessionIsLive(sessionPath), {
    pid,
    timeoutMs,
    missingPidMessage: "Guru Terminal launcher exited before the session was ready",
    timeoutMessage: `Guru Terminal session was not ready within ${timeoutMs} milliseconds`,
  });
  process.exit(0);
}
