import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { registerWorkspaceTools } from "./workbench-tools.mjs";

function registerTools(skillFiles = new Map(), brokerRequest = async () => {
  throw new Error("unexpected broker request");
}) {
  const tools = new Map();
  registerWorkspaceTools(
    {
      registerTool: (tool) => tools.set(tool.name, tool),
    },
    skillFiles,
    brokerRequest,
  );
  return tools;
}

test("ls, find, and grep use their Rust broker methods and expose delivered result refs", async () => {
  const calls = [];
  const signal = new AbortController().signal;
  const brokerRequest = async (method, params, receivedSignal) => {
    calls.push({ method, params, signal: receivedSignal });
    return {
      text: method === "workbench.grep" ? "notes/alpha.md:2:match" : "notes/alpha.md",
      count: 1,
      truncated: false,
      ...(method === "workbench.grep"
        ? { skipped_binary: 0, skipped_binary_paths: [], warnings: [] }
        : {}),
      result_ref: `result:${method}`,
    };
  };
  const tools = registerTools(new Map(), brokerRequest);

  const listed = await tools.get("ls").execute("ls", { path: "notes", limit: 10 }, signal);
  const found = await tools.get("find").execute("find", { pattern: "**/*.md" }, signal);
  const searched = await tools.get("grep").execute(
    "grep",
    { pattern: "match", glob: "**/*.md", context: 1 },
    signal,
  );

  assert.deepEqual(
    calls.map(({ method, params }) => ({ method, params })),
    [
      { method: "workbench.ls", params: { path: "notes", limit: 10 } },
      { method: "workbench.find", params: { pattern: "**/*.md" } },
      {
        method: "workbench.grep",
        params: { pattern: "match", glob: "**/*.md", context: 1 },
      },
    ],
  );
  assert(calls.every((call) => call.signal === signal));
  assert.match(listed.content[0].text, /result:workbench\.ls/);
  assert.match(found.content[0].text, /result:workbench\.find/);
  assert.match(searched.content[0].text, /notes\/alpha\.md:2:match/);
  assert.match(searched.content[0].text, /result:workbench\.grep/);
  assert.equal(searched.details.result_ref, "result:workbench.grep");
  assert.equal(searched.details.text, undefined);
});

test("enabled Skill instruction reads remain local control material", async (t) => {
  const temporary = mkdtempSync(join(tmpdir(), "guruterminal-workbench-skill-"));
  const workbench = join(temporary, "workbench");
  const skill = join(temporary, "SKILL.md");
  const previousCwd = process.cwd();
  mkdirSync(workbench, { mode: 0o700 });
  writeFileSync(skill, "# Exact enabled instructions\n", { mode: 0o600 });
  process.chdir(workbench);
  t.after(() => {
    process.chdir(previousCwd);
    rmSync(temporary, { recursive: true, force: true });
  });

  let brokerCalls = 0;
  const tools = registerTools(
    new Map([[skill, realpathSync(skill)]]),
    async () => {
      brokerCalls += 1;
      throw new Error("Skill reads must not reach the result broker");
    },
  );
  const result = await tools.get("read").execute("read-skill", { path: skill });

  assert.equal(result.content[0].text, "# Exact enabled instructions\n");
  assert.equal(result.details.access, "enabled_skill");
  assert.equal(result.details.result_ref, undefined);
  assert.equal(brokerCalls, 0);
});

test("workbench text tools reject broker responses without a committed result ref", async () => {
  const tools = registerTools(new Map(), async () => ({ text: "uncommitted", count: 1 }));
  await assert.rejects(
    () => tools.get("ls").execute("ls", {}),
    /malformed workbench result/,
  );
});
