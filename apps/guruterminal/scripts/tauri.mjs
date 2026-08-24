#!/usr/bin/env node
import { spawn } from "node:child_process";
import { mkdirSync, unlinkSync, existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  resolveWebdriverPort,
  writeSession,
} from "../e2e/wait-session-lib.mjs";

const appRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const cli = resolve(appRoot, "node_modules/@tauri-apps/cli/tauri.js");
const sessionPath = resolve(appRoot, "e2e/artifacts/current-session.json");
const args = process.argv.slice(2);
const isDev = args[0] === "dev";
const hasFeatures = args.includes("--features");
const preferred = Number(process.env.TAURI_WEBDRIVER_PORT || 14440);

if (isDev && !hasFeatures) {
  args.push("--features", "webdriver");
}

const env = { ...process.env };
if (isDev) {
  const port = await resolveWebdriverPort(preferred);
  env.TAURI_WEBDRIVER_PORT = String(port);
  mkdirSync(dirname(sessionPath), { recursive: true, mode: 0o700 });
  writeSession(sessionPath, process.pid, port, "development");
}

const child = spawn(process.execPath, [cli, ...args], {
  stdio: "inherit",
  env,
});

const clearSession = () => {
  if (!isDev || !existsSync(sessionPath)) return;
  try {
    const session = JSON.parse(readFileSync(sessionPath, "utf8"));
    if (session.launcherPid === process.pid) unlinkSync(sessionPath);
  } catch {
    // Leave a session owned by another launcher alone.
  }
};

for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
  process.on(signal, () => {
    child.kill(signal);
  });
}

child.on("exit", (code, signal) => {
  clearSession();
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }
  process.exit(code ?? 1);
});
