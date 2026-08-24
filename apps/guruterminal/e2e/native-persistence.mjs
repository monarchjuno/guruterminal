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

const { displayed, clickButton, waitForText } = createWebdriverHelpers(
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

async function assertImportedMemory() {
  await navigateTo("Memory");
  const library = await displayed("#main-panel-library");
  await waitForText(library, "4 pages.");
  for (const record of IMPORTED_RECORDS) await waitForText(library, record.title);

  const search = await displayed('input[placeholder="Search memory"]');
  for (const record of IMPORTED_RECORDS) {
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
    await search.setValue(record.query);
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
    await result.click();
    await waitForText(library, record.detail);
    assert.equal(await result.getAttribute("aria-current"), "page");
  }
  await search.setValue("");
  await clickButton("All types", library);
  await waitForText(library, "4 pages.");
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
    await browser.saveScreenshot(resolve(artifactRoot, "native-persistence-seed.png"));
    console.log("Native persistence seed with Memory import passed.");
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
    await assertImportedMemory();
    await browser.saveScreenshot(resolve(artifactRoot, "native-persistence-verify.png"));
    console.log("Native persistence restart with imported Memory passed.");
  }
} catch (error) {
  await browser.saveScreenshot(
    resolve(artifactRoot, `native-persistence-${phase}-failure.png`),
  );
  throw error;
} finally {
  await browser.deleteSession();
}
