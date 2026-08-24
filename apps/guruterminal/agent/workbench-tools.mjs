import { lstatSync, readFileSync, realpathSync } from "node:fs";
import { resolve } from "node:path";

import { requestBroker } from "./broker-client.mjs";

const MAX_WORKBENCH_FILE_BYTES = 512 * 1024;
const MAX_SKILL_FILE_BYTES = 64 * 1024;
const MAX_SKILL_FILES = 64;
const MAX_TOOL_OUTPUT_BYTES = 50 * 1024;
const MAX_GREP_CONTEXT = 3;

export function loadSkillFiles() {
  const raw = process.env.GURUTERMINAL_SKILL_FILES;
  delete process.env.GURUTERMINAL_SKILL_FILES;
  if (raw === undefined) return new Map();
  if (Buffer.byteLength(raw, "utf8") > 16 * 1024 || raw.includes("\0")) {
    throw new Error("Guru Terminal skill allowlist is invalid");
  }
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error("Guru Terminal skill allowlist is invalid");
  }
  if (!Array.isArray(parsed) || parsed.length > MAX_SKILL_FILES) {
    throw new Error("Guru Terminal skill allowlist is invalid");
  }
  const paths = new Map();
  for (const input of parsed) {
    if (typeof input !== "string" || !input || input.includes("\0")) {
      throw new Error("Guru Terminal skill allowlist is invalid");
    }
    const path = resolve(input);
    const metadata = lstatSync(path);
    const canonical = realpathSync(path);
    if (
      metadata.isSymbolicLink() ||
      !metadata.isFile() ||
      metadata.size <= 0 ||
      metadata.size > MAX_SKILL_FILE_BYTES ||
      paths.has(path) ||
      [...paths.values()].includes(canonical)
    ) {
      throw new Error("Guru Terminal skill allowlist is invalid");
    }
    paths.set(path, canonical);
  }
  return paths;
}

function textResult(text, details = {}) {
  return {
    content: [{ type: "text", text }],
    details,
  };
}

function jsonResult(value) {
  return {
    content: [{ type: "text", text: JSON.stringify(value, null, 2) }],
    details: value,
  };
}

function boundedText(text) {
  const bytes = Buffer.from(text, "utf8");
  if (bytes.length <= MAX_TOOL_OUTPUT_BYTES) return text;
  return `${bytes.subarray(0, MAX_TOOL_OUTPUT_BYTES).toString("utf8")}\n\n[Output truncated at 50KB]`;
}

function brokeredTextResult(value) {
  const { text, ...details } = value;
  if (typeof text !== "string" || typeof details.result_ref !== "string") {
    throw new Error("Tool broker returned a malformed workbench result");
  }
  return textResult(`${text}\n\n[result_ref: ${details.result_ref}]`, details);
}

export function registerWorkspaceTools(pi, skillFiles, brokerRequest = requestBroker) {
  pi.registerTool({
    name: "read",
    label: "Read workbench file",
    description: "Read a UTF-8 text file inside this Guru's persistent workbench or one exact host-enabled Skill file. Workbench reads return an opaque revision token bound to the canonical relative path and exact bytes; pass that token as expected_revision when replacing or editing the same file.",
    parameters: {
      type: "object",
      additionalProperties: false,
      required: ["path"],
      properties: {
        path: { type: "string", minLength: 1 },
        offset: { type: "integer", minimum: 1 },
        limit: { type: "integer", minimum: 1, maximum: 2_000 },
      },
    },
    async execute(_id, input, signal) {
      const requested = resolve(input.path);
      if (skillFiles.has(requested)) {
        const metadata = lstatSync(requested);
        if (
          metadata.isSymbolicLink() ||
          !metadata.isFile() ||
          metadata.size > MAX_SKILL_FILE_BYTES ||
          realpathSync(requested) !== skillFiles.get(requested)
        ) {
          throw new Error("Enabled Skill file is invalid");
        }
        const lines = readFileSync(requested, "utf8").split("\n");
        const start = (input.offset ?? 1) - 1;
        const selected = lines.slice(start, start + (input.limit ?? 2_000));
        return textResult(boundedText(selected.join("\n")), {
          path: requested,
          totalLines: lines.length,
          access: "enabled_skill",
        });
      }
      const params = { path: input.path };
      if (input.offset !== undefined) params.offset = input.offset;
      if (input.limit !== undefined) params.limit = input.limit;
      return jsonResult(await brokerRequest("workbench.read", params, signal));
    },
  });

  pi.registerTool({
    name: "write",
    label: "Write workbench file",
    description: "Create or replace a UTF-8 text file inside this Guru's persistent workbench. Omit expected_revision to create a new file; replacing an existing file requires the revision from the last read. App-owned attachment snapshots are read-only. A revision conflict leaves the original bytes unchanged and returns the current revision.",
    parameters: {
      type: "object",
      additionalProperties: false,
      required: ["path", "content"],
      properties: {
        path: { type: "string", minLength: 1 },
        content: { type: "string", maxLength: MAX_WORKBENCH_FILE_BYTES },
        expected_revision: { type: "string", minLength: 64, maxLength: 64 },
      },
    },
    async execute(_id, input, signal) {
      const params = { path: input.path, content: input.content };
      if (input.expected_revision !== undefined) {
        params.expected_revision = input.expected_revision;
      }
      return jsonResult(await brokerRequest("workbench.write", params, signal));
    },
  });

  pi.registerTool({
    name: "edit",
    label: "Edit workbench file",
    description: "Replace one exact text occurrence in a file inside this Guru's workbench. expected_revision from the last read is required. App-owned attachment snapshots are read-only. A revision conflict leaves the original bytes unchanged and returns the current revision.",
    parameters: {
      type: "object",
      additionalProperties: false,
      required: ["path", "old_text", "new_text", "expected_revision"],
      properties: {
        path: { type: "string", minLength: 1 },
        old_text: { type: "string", minLength: 1 },
        new_text: { type: "string" },
        expected_revision: { type: "string", minLength: 64, maxLength: 64 },
      },
    },
    async execute(_id, input, signal) {
      return jsonResult(await brokerRequest("workbench.edit", {
        path: input.path,
        old_text: input.old_text,
        new_text: input.new_text,
        expected_revision: input.expected_revision,
      }, signal));
    },
  });

  pi.registerTool({
    name: "ls",
    label: "List workbench directory",
    description: "List files and directories inside this Guru's workbench. Every successfully delivered result receives a turn-local result_ref for Evidence or Chart use.",
    parameters: {
      type: "object",
      additionalProperties: false,
      properties: {
        path: { type: "string", maxLength: 4_096 },
        limit: { type: "integer", minimum: 1, maximum: 500 },
      },
    },
    async execute(_id, input, signal) {
      return brokeredTextResult(await brokerRequest("workbench.ls", input, signal));
    },
  });

  pi.registerTool({
    name: "find",
    label: "Find workbench files",
    description: "Find workbench files and directories by glob pattern. Every successfully delivered result receives a turn-local result_ref for Evidence or Chart use.",
    parameters: {
      type: "object",
      additionalProperties: false,
      required: ["pattern"],
      properties: {
        pattern: { type: "string", minLength: 1, maxLength: 4_096 },
        path: { type: "string", maxLength: 4_096 },
        limit: { type: "integer", minimum: 1, maximum: 200 },
      },
    },
    async execute(_id, input, signal) {
      return brokeredTextResult(await brokerRequest("workbench.find", input, signal));
    },
  });

  pi.registerTool({
    name: "grep",
    label: "Search workbench text",
    description: "Search bounded UTF-8 workbench files with a regular expression. Optional context includes 0-3 surrounding lines. Binary-like files are skipped with a bounded warning. Walk, file, and output caps are unchanged. Every successfully delivered result receives a turn-local result_ref for Evidence or Chart use.",
    parameters: {
      type: "object",
      additionalProperties: false,
      required: ["pattern"],
      properties: {
        pattern: { type: "string", minLength: 1, maxLength: 4_096 },
        path: { type: "string", maxLength: 4_096 },
        glob: { type: "string", minLength: 1, maxLength: 4_096 },
        limit: { type: "integer", minimum: 1, maximum: 200 },
        context: { type: "integer", minimum: 0, maximum: MAX_GREP_CONTEXT },
      },
    },
    async execute(_id, input, signal) {
      return brokeredTextResult(await brokerRequest("workbench.grep", input, signal));
    },
  });
}
