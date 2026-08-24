import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const helper = join(dirname(fileURLToPath(import.meta.url)), "detach-launch.mjs");

test("detached launcher stays alive after the parent helper exits", async (t) => {
  const directory = mkdtempSync(join(tmpdir(), "guruterminal-detach-"));
  t.after(() => {
    rmSync(directory, { recursive: true, force: true });
  });
  const marker = join(directory, "pid");
  const log = join(directory, "log");
  const script = join(directory, "child.sh");
  writeFileSync(
    script,
    `#!/bin/sh
sleep 0.2
printf '%s %s\\n' "$$" "$PPID" > "${marker}"
while true; do sleep 1; done
`,
    { mode: 0o700 },
  );

  const childPid = Number(
    execFileSync(process.execPath, [helper, script, log], {
      encoding: "utf8",
    }).trim(),
  );
  assert.ok(Number.isInteger(childPid) && childPid > 0);
  t.after(() => {
    try {
      process.kill(childPid, "TERM");
    } catch {
      // already gone
    }
  });

  let launchedPid;
  let parentPid;
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      const [pid, ppid] = readFileSync(marker, "utf8").trim().split(/\s+/);
      launchedPid = Number(pid);
      parentPid = Number(ppid);
      if (
        Number.isInteger(launchedPid) &&
        launchedPid > 0 &&
        Number.isInteger(parentPid) &&
        parentPid >= 0
      ) {
        break;
      }
    } catch {
      // The child has not written the marker yet.
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  assert.equal(launchedPid, childPid);
  assert.equal(parentPid, 1);
  process.kill(childPid, 0);
});
