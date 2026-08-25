import assert from "node:assert/strict";
import { mkdir, readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { remote } from "webdriverio";
import { createWebdriverHelpers } from "./webdriver-helpers.mjs";

const e2eRoot = dirname(fileURLToPath(import.meta.url));
const [sessionPath, phase] = process.argv.slice(2);
assert.ok(sessionPath, "usage: node native-persistence.mjs <session.json> <seed|verify>");
assert.ok(["seed", "verify"].includes(phase), "phase must be seed or verify");

const session = JSON.parse(await readFile(sessionPath, "utf8"));
const browser = await remote({
  ...session.webdriverConfig,
  capabilities: session.capabilities,
  logLevel: "error",
});
const artifactRoot = resolve(e2eRoot, "artifacts");
await mkdir(artifactRoot, { recursive: true });

const {
  bodyText,
  childWithText,
  displayed,
  clickButton,
  waitForText,
  waitForTextGone,
} = createWebdriverHelpers(
  browser,
  { defaultTimeout: 10_000, bodyTextLimit: 16_000 },
);

const IMPORTED_AGENT = "Imported Memory E2E";
const IMPORTED_RECORDS = [
  {
    title: "Imported Quality Covenant",
    kind: "Wiki",
    query: "quality covenant",
    detail: "cash conversion above reported earnings",
  },
  {
    title: "Imported Risk Discipline",
    kind: "Lens",
    query: "risk discipline",
    detail: "before changing a position size",
  },
  {
    title: "Imported Cash Observation",
    kind: "Evidence",
    query: "cash observation",
    detail: "2026-Q2 conversion check",
  },
  {
    title: "Imported Allocation Review",
    kind: "Decision",
    query: "allocation review",
    detail: "downside review ahead of a larger allocation",
  },
];
const IMPORTED_WIKI = IMPORTED_RECORDS[0];
const DELETED_IMPORTED_RECORD = IMPORTED_RECORDS[3];
const REMAINING_IMPORTED_RECORDS = IMPORTED_RECORDS.filter(
  (record) => record !== DELETED_IMPORTED_RECORD,
);
const IMPORTED_WIKI_BODY = `# Covenant

The imported quality covenant keeps cash conversion above reported earnings before a valuation multiple can expand.`;
const IMPORTED_WIKI_MARKDOWN = `---
id: wiki:import/quality-covenant
title: Imported Quality Covenant
summary: A durable cash conversion covenant for the Native Memory import fixture.
as_of: 2026-08-20T00:00:00Z
entities:
  - Native Import Co.
tags:
  - import-fixture
  - quality
---

# Covenant

The imported quality covenant keeps cash conversion above reported earnings before a valuation multiple can expand.`;
const IMPORTED_WIKI_EDIT_MARKER = "Native persistence Wiki edit marker";
const MEMORY_WRITE_TIMEOUT_MS = 60_000;

async function navigateTo(text) {
  for (const button of await browser.$$("button")) {
    if ((await button.isDisplayed()) && (await button.getText()).trim() === text) {
      await button.click();
      return;
    }
  }
  await (await displayed('button[data-sidebar="trigger"]')).click();
  await clickButton(text);
}

async function selectImportedAgent() {
  const agents = await displayed("#main-panel-agents");
  const findImportedAgent = async () => {
    for (const button of await agents.$$(".agents-list-item")) {
      if (
        (await button.isDisplayed()) &&
        (await button.getText()).trim().startsWith(IMPORTED_AGENT)
      ) {
        return button;
      }
    }
    return null;
  };
  await browser.waitUntil(async () => Boolean(await findImportedAgent()), {
    timeout: 10_000,
    interval: 100,
    timeoutMsg: `Imported agent did not appear: ${IMPORTED_AGENT}`,
  });
  const imported = await findImportedAgent();
  assert.ok(imported, `Imported agent disappeared: ${IMPORTED_AGENT}`);
  await imported.click();
  await browser.waitUntil(
    async () => {
      const current = await findImportedAgent();
      return (await current?.getAttribute("data-active")) === "true";
    },
    {
      timeout: 10_000,
      interval: 100,
      timeoutMsg: `Imported agent was not selected: ${IMPORTED_AGENT}`,
    },
  );
}

async function waitForLibraryIdle(library, timeout = 10_000) {
  const workspace = await displayed("#main-panel-library .library-page", timeout);
  try {
    await browser.waitUntil(
      async () =>
        (await workspace.isDisplayed()) &&
        (await workspace.getAttribute("aria-busy")) === "false",
      {
        timeout,
        interval: 100,
        timeoutMsg: "Memory workspace did not become idle",
      },
    );
  } catch (cause) {
    throw new Error(
      `Memory workspace did not become idle\n${await bodyText()}`,
      { cause },
    );
  }
}

async function waitForImportedMemorySelection(library, record) {
  const selector = `button[aria-label^="Open ${record.title} ("]`;
  await browser.waitUntil(
    async () => {
      const labels = [];
      for (const button of await library.$$("button[data-library-result]")) {
        if (await button.isDisplayed()) {
          labels.push(await button.getAttribute("aria-label"));
        }
      }
      return labels.length === 1 && labels[0]?.startsWith(`Open ${record.title} (`);
    },
    {
      timeout: 10_000,
      interval: 100,
      timeoutMsg: `Search did not return only ${record.title}`,
    },
  );
  const result = await displayed(selector);
  // LibraryView automatically opens its sole search result. Waiting for that
  // transition avoids starting a second overlapping read by clicking it again.
  await waitForText(library, record.detail);
  await waitForLibraryIdle(library);
  assert.equal(await result.getAttribute("aria-current"), "page");
}

async function waitForMemoryEditorToClose(timeout = MEMORY_WRITE_TIMEOUT_MS) {
  try {
    await browser.waitUntil(
      async () => {
        for (const editor of await browser.$$('[aria-label="Edit memory"]')) {
          if (await editor.isDisplayed()) return false;
        }
        return true;
      },
      {
        timeout,
        interval: 100,
        timeoutMsg: "Memory editor did not close after save",
      },
    );
  } catch (cause) {
    throw new Error(
      `Memory editor did not close after save\n${await bodyText()}`,
      { cause },
    );
  }
}

async function assertImportedMemory(records = IMPORTED_RECORDS) {
  await navigateTo("Memory");
  const library = await displayed("#main-panel-library");
  await waitForText(library, `${records.length} pages.`);
  for (const record of records) await waitForText(library, record.title);
  await waitForLibraryIdle(library);

  const search = await displayed('input[placeholder="Search memory"]');
  for (const record of records) {
    const kindFilter = await displayed(`button[aria-label="${record.kind}"]`);
    await kindFilter.click();
    await browser.waitUntil(
      async () => (await kindFilter.getAttribute("aria-pressed")) === "true",
      {
        timeout: 10_000,
        interval: 100,
        timeoutMsg: `${record.kind} filter was not selected`,
      },
    );
    await waitForLibraryIdle(library);
    await search.setValue(record.query);
    await waitForImportedMemorySelection(library, record);
  }
  await search.setValue("");
  await waitForLibraryIdle(library);
  await clickButton("All types", library);
  await waitForText(library, `${records.length} pages.`);
  await waitForLibraryIdle(library);
}

async function openImportedMemory(record) {
  await navigateTo("Memory");
  const library = await displayed("#main-panel-library");
  await waitForLibraryIdle(library);
  const search = await displayed('input[placeholder="Search memory"]');
  const kindFilter = await displayed(`button[aria-label="${record.kind}"]`);
  if ((await kindFilter.getAttribute("aria-pressed")) !== "true") {
    await kindFilter.click();
    await waitForLibraryIdle(library);
  }
  await search.setValue(record.query);
  await waitForImportedMemorySelection(library, record);
  return library;
}

async function assertImportedWikiIsRestored() {
  const library = await openImportedMemory(IMPORTED_WIKI);
  await clickButton("Raw", library);
  const raw = await displayed("pre.raw-markdown");
  const markdown = (await raw.getText()).trim();
  assert.equal(markdown, IMPORTED_WIKI_MARKDOWN);
  assert.equal(markdown.includes(IMPORTED_WIKI_EDIT_MARKER), false);
  await clickButton("Rendered", library);
  await waitForText(library, IMPORTED_WIKI.detail);
}

async function editAndRevertImportedWiki() {
  const library = await openImportedMemory(IMPORTED_WIKI);
  await clickButton("Edit", library);
  const editor = await displayed('[aria-label="Edit memory"]');
  // Entering edit mode refreshes LibraryView's search. Let that reader drain
  // before starting the exclusive Memory write below.
  await waitForLibraryIdle(library);
  const body = await displayed("textarea.memory-markdown-editor");
  assert.equal(await body.getValue(), IMPORTED_WIKI_BODY);
  await body.setValue(`${IMPORTED_WIKI_BODY}\n\n${IMPORTED_WIKI_EDIT_MARKER}`);
  const expectedBody = `${IMPORTED_WIKI_BODY}\n\n${IMPORTED_WIKI_EDIT_MARKER}`;
  await browser.waitUntil(async () => (await body.getValue()) === expectedBody, {
    timeout: 10_000,
    interval: 100,
    timeoutMsg: "Wiki editor did not retain the revised body",
  });
  const save = await childWithText(editor, "button", "Save memory");
  await save.waitForEnabled({ timeout: 10_000 });
  await save.click();
  await waitForMemoryEditorToClose();
  await waitForLibraryIdle(library, MEMORY_WRITE_TIMEOUT_MS);
  await childWithText(library, "button", "Revert", MEMORY_WRITE_TIMEOUT_MS);
  await waitForText(library, IMPORTED_WIKI_EDIT_MARKER, MEMORY_WRITE_TIMEOUT_MS);
  await clickButton("Revert", library);
  await waitForTextGone(library, IMPORTED_WIKI_EDIT_MARKER, 20_000);
  await assertImportedWikiIsRestored();
}

async function deleteImportedMemory(record) {
  const library = await openImportedMemory(record);
  await clickButton("Delete", library);

  let confirmation = "";
  await browser.waitUntil(
    async () => {
      try {
        confirmation = await browser.getAlertText();
        return true;
      } catch {
        return false;
      }
    },
    {
      timeout: 10_000,
      interval: 100,
      timeoutMsg: "Memory delete confirmation did not appear",
    },
  );
  assert.equal(confirmation, `Delete “${record.title}”?`);
  await browser.acceptAlert();

  await waitForText(library, `${REMAINING_IMPORTED_RECORDS.length} pages.`, 20_000);
  await waitForText(library, "No matching memories", 20_000);
  await waitForTextGone(library, record.title, 20_000);
}

async function assertImportedMemoryIsDeleted(record) {
  await navigateTo("Memory");
  const library = await displayed("#main-panel-library");
  await waitForText(library, `${REMAINING_IMPORTED_RECORDS.length} pages.`);
  const search = await displayed('input[placeholder="Search memory"]');
  const kindFilter = await displayed(`button[aria-label="${record.kind}"]`);
  if ((await kindFilter.getAttribute("aria-pressed")) !== "true") {
    await kindFilter.click();
  }
  await search.setValue(record.query);
  await browser.waitUntil(
    async () => {
      for (const button of await library.$$("button[data-library-result]")) {
        if (
          (await button.isDisplayed()) &&
          (await button.getAttribute("aria-label"))?.startsWith(`Open ${record.title} (`)
        ) {
          return false;
        }
      }
      return true;
    },
    {
      timeout: 10_000,
      interval: 100,
      timeoutMsg: `Deleted Memory record returned in search: ${record.title}`,
    },
  );
  await waitForText(library, "No matching memories");
  await search.setValue("");
  await clickButton("All types", library);
  await waitForText(library, `${REMAINING_IMPORTED_RECORDS.length} pages.`);
}

try {
  if (phase === "seed") {
    const onboarding = await displayed("main");
    await waitForText(onboarding, "Connect a model provider");
    await clickButton("Agents");
    const agents = await displayed("#main-panel-agents");
    await waitForText(agents, "My Agent");
    await clickButton("Rename", agents);
    await (await displayed("#rename-guru-name")).setValue("Persistent E2E Agent");
    await clickButton("Save", await displayed('[role="dialog"]'));
    await waitForText(agents, "Persistent E2E Agent");

    await clickButton("Chat");
    await (await displayed('button[aria-label="New session for Persistent E2E Agent"]')).click();
    await browser.waitUntil(
      async () => (await browser.$$('button[aria-label="Rename session"]')).length === 1,
      { timeout: 10_000, interval: 100, timeoutMsg: "Persistent session was not created" },
    );
    const [rename] = await browser.$$('button[aria-label="Rename session"]');
    await rename.moveTo();
    await rename.click();
    const dialog = await displayed('[role="dialog"]');
    await (await displayed("#rename-thread-name")).setValue("Persistent session");
    await clickButton("Save", dialog);
    await waitForText(await displayed('[aria-label="Application navigation"]'), "Persistent session");
    await clickButton("Agents");
    const agentsAfterSession = await displayed("#main-panel-agents");
    await clickButton("Import", agentsAfterSession);
    await waitForText(agentsAfterSession, IMPORTED_AGENT);
    await selectImportedAgent();
    await assertImportedMemory();
    await editAndRevertImportedWiki();
    await deleteImportedMemory(DELETED_IMPORTED_RECORD);
    await assertImportedMemoryIsDeleted(DELETED_IMPORTED_RECORD);
    await assertImportedMemory(REMAINING_IMPORTED_RECORDS);
    await browser.saveScreenshot(resolve(artifactRoot, "native-persistence-seed.png"));
    console.log("Native persistence seed with Memory import edit, Revert, and delete passed.");
  } else {
    const navigation = await displayed('[aria-label="Application navigation"]');
    await waitForText(navigation, "Persistent E2E Agent");
    await (await displayed('button[title="Persistent E2E Agent"]')).click();
    await waitForText(navigation, "Persistent session");
    await clickButton("Agents");
    const agents = await displayed("#main-panel-agents");
    await waitForText(agents, "Persistent E2E Agent");
    await clickButton("Chat");
    assert.equal(await (await displayed("#main-tab-chat")).getAttribute("aria-current"), "page");
    await waitForText(await displayed('[aria-label="Application navigation"]'), "Persistent session");
    const onboarding = await displayed("main");
    await waitForText(onboarding, "Connect a model provider");
    assert.equal(
      (await browser.$$('textarea[aria-label="Message Guru"]')).length,
      0,
      "Chat composer must remain unavailable until a provider is connected",
    );
    await clickButton("Agents");
    await selectImportedAgent();
    await assertImportedMemory(REMAINING_IMPORTED_RECORDS);
    await assertImportedWikiIsRestored();
    await assertImportedMemoryIsDeleted(DELETED_IMPORTED_RECORD);
    await browser.saveScreenshot(resolve(artifactRoot, "native-persistence-verify.png"));
    console.log("Native persistence restart with reverted and deleted imported Memory passed.");
  }
} catch (error) {
  await browser.saveScreenshot(
    resolve(artifactRoot, `native-persistence-${phase}-failure.png`),
  );
  throw error;
} finally {
  await browser.deleteSession();
}
