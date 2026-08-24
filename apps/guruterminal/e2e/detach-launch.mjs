#!/usr/bin/env node
import { spawn } from "node:child_process";
import { openSync } from "node:fs";

const script = process.argv[2];
const logPath = process.argv[3];
if (!script || !logPath) {
  console.error("Usage: node detach-launch.mjs <script> <log>");
  process.exit(2);
}

const logFd = openSync(logPath, "a");
const child = spawn(script, [], {
  detached: true,
  stdio: ["ignore", logFd, logFd],
  env: {
    ...process.env,
    GURUTERMINAL_E2E_DETACH: "1",
  },
});
child.once("error", (error) => {
  console.error(error.message);
  process.exit(1);
});
if (!Number.isInteger(child.pid) || child.pid <= 0) {
  console.error("detached launcher did not start");
  process.exit(1);
}
process.stdout.write(String(child.pid));
child.unref();
