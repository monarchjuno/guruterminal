import assert from "node:assert/strict";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { remote } from "webdriverio";
import { createWebdriverHelpers } from "./webdriver-helpers.mjs";
import {
  collectWorkProgress,
  compactionObservation,
  expandProgress,
  failedCompactRows,
  isVisible,
  missingHarnessActions,
  textContainsCompactionEnableFailure,
} from "./work-progress.mjs";

const e2eRoot = dirname(fileURLToPath(import.meta.url));
const sessionPath = process.argv[2];
const phase = process.argv[3];
assert.ok(
  sessionPath,
  "usage: node native-live-chat.mjs <current-session.json> <run|verify|smoke|artifact|artifact-memory|artifact-after-finance|artifact-after-finance-warm|artifact-after-finance-no-history>",
);
assert.ok(
  [
    "run",
    "verify",
    "smoke",
    "artifact",
    "artifact-memory",
    "artifact-after-finance",
    "artifact-after-finance-warm",
    "artifact-after-finance-no-history",
  ].includes(phase),
  "phase must be run, verify, smoke, artifact, artifact-memory, artifact-after-finance, artifact-after-finance-warm, or artifact-after-finance-no-history",
);

const session = JSON.parse(await readFile(sessionPath, "utf8"));
const browser = await remote({
  ...session.webdriverConfig,
  capabilities: session.capabilities,
  logLevel: "error",
});
const artifactRoot = resolve(e2eRoot, "artifacts");
await mkdir(artifactRoot, { recursive: true });
const artifactTitle = "Luna Artifact E2E";
const artifactToken = "LUNA-ARTIFACT-E2E";
const lineageChartTitle = "Native Lineage Chart";
const lineageEvidenceTitle = "Native Lineage Evidence";
const lineageDecisionTitle = "The deterministic lineage flow completed.";
const lineageToken = "NATIVE-LINEAGE-E2E-COMPLETE";
const followupToken = "LUNA-NATIVE-E2E-FOLLOWUP";
const financeResultToken = "LUNA-FINANCE-CORE-25-PERCENT-E2E";
const worldBankResultToken = "LUNA-WORLD-BANK-MACRO-E2E";
const openbbResultToken = "LUNA-OPENBB-KEYLESS-E2E";
const openbbEvidenceTitle = "OpenBB keyless AAPL quote receipt";
const webResearchResultToken = "LUNA-WEB-RESEARCH-E2E";
const restartedMemoryToken = "LUNA-RESTARTED-MEMORY-E2E";
const wikiTitle = "WP4 cobalt-foil spare-capacity rule";
const lensTitle = "WP4 native-e2e capital-cycle lens";
// Luna/max can spend several minutes planning the first tool call after a
// deliberately long, multi-capability transcript. Keep this above Pi's
// default five-minute provider idle policy: it is a live acceptance budget,
// not a product-side liveness deadline.
const FIRST_TOOL_PROGRESS_TIMEOUT_MS = 360_000;
// Max-thinking models can legitimately produce no user-visible token before
// the provider's five-minute idle boundary. This is an acceptance budget, not
// a product-side watchdog.
const FIRST_VISIBLE_ASSISTANT_DELTA_TIMEOUT_MS = 360_000;
const acceptanceObservations = {
  streamedAssistantDelta: null,
  financeCore: null,
  worldBankMacro: null,
  openbbKeyless: null,
  webResearch: null,
  artifact: null,
  restartedMemoryReuse: null,
};

const {
  bodyText,
  displayed,
  clickButton,
  waitForText,
  waitForAppReady,
} = createWebdriverHelpers(browser, {
  defaultTimeout: 15_000,
  bodyTextLimit: 20_000,
  interval: 150,
});

async function selectOption(triggerSelector, matcher, timeout = 30_000) {
  await (await displayed(triggerSelector)).click();
  await browser.waitUntil(
    async () => {
      for (const option of await browser.$$('[role="option"]')) {
        if ((await option.isDisplayed()) && matcher((await option.getText()).trim())) return true;
      }
      return false;
    },
    {
      timeout,
      interval: 150,
      timeoutMsg: `Timed out waiting for a matching option\n${await bodyText()}`,
    },
  );
  for (const option of await browser.$$('[role="option"]')) {
    if ((await option.isDisplayed()) && matcher((await option.getText()).trim())) {
      await option.click();
      return;
    }
  }
  throw new Error("Matching option disappeared");
}

async function createLiveAgent() {
  await displayed("main");
  await waitForAppReady();
  await clickButton("Agents");
  const panel = await displayed("#main-panel-agents");
  await waitForText(panel, "My Agent");
  await clickButton("Rename", panel);
  await (await displayed("#rename-guru-name")).setValue("Luna Native Agent");
  await clickButton("Save", await displayed('[role="dialog"]'));
  await waitForText(panel, "Luna Native Agent");
}

async function connectExistingPiProfile() {
  await clickButton("Settings");
  const panel = await displayed("#main-panel-settings");
  const alreadyConnected = await browser.waitUntil(
    async () => {
      const text = await panel.getText();
      return /OpenAI with ChatGPT/i.test(text) && /\bConnected\b/i.test(text);
    },
    {
      timeout: 15_000,
      interval: 150,
      timeoutMsg: `Timed out waiting for OpenAI to appear as connected\n${await bodyText()}`,
    },
  ).then(() => true).catch(() => false);
  if (!alreadyConnected) {
    await clickButton("Connect provider", panel);
    await selectOption(
      '[aria-label="Provider"]',
      (label) => label.startsWith("OpenAI with ChatGPT"),
    );
    await waitForText(panel, "Connected", 15_000);
  }
  await waitForText(panel, "GPT-5.6 Luna", 90_000);
  await clickButton("Show all", panel);
  for (const button of await browser.$$("button")) {
    if ((await button.isDisplayed()) && (await button.getText()).trim() === "Close") {
      await button.click();
      break;
    }
  }
  await clickButton("Back to app");
}

async function clickMenuRadio(exactText) {
  let match = null;
  const observed = new Set();
  try {
    await browser.waitUntil(async () => {
      for (const menu of await browser.$$('[data-slot="dropdown-menu-content"]')) {
        if (!(await isVisible(menu))) continue;
        for (const item of await menu.$$('[role="menuitemradio"]')) {
          let label = "";
          try {
            label = (await item.getText()).trim();
          } catch {
            continue;
          }
          if (observed.size < 40) {
            observed.add(
              `${label}:${(await isVisible(item)) ? "visible" : "clipped"}`,
            );
          }
          if (label !== exactText) continue;
          match = item;
          return true;
        }
      }
      return false;
    }, {
      timeout: 5_000,
      interval: 125,
      timeoutMsg: `Timed out waiting for visible menu item: ${exactText}`,
    });
  } catch (error) {
    throw new Error(
      `${error.message}; observed model menu items: ${[...observed].join(" | ") || "none"}`,
    );
  }
  if (!match) {
    throw new Error(`Model menu item disappeared: ${exactText}\n${await bodyText()}`);
  }
  if (await isVisible(match)) {
    await match.click();
    return;
  }

  // Radix supports type-ahead selection. It avoids WebKit's flaky async
  // script execution when an otherwise-rendered menu item is clipped inside
  // the scroll container.
  await browser.keys(exactText);
  await browser.keys("Enter");
}

async function visibleModelMenu() {
  for (const menu of await browser.$$('[data-slot="dropdown-menu-content"]')) {
    if (await isVisible(menu)) return menu;
  }
  return null;
}

async function openModelMenu(modelMenu) {
  const waitForMenu = async () => {
    try {
      return await browser.waitUntil(async () => (await visibleModelMenu()) || false, {
        timeout: 1_500,
        interval: 100,
        timeoutMsg: "Chat model menu did not become visible",
      });
    } catch {
      return null;
    }
  };
  await modelMenu.click();
  let menu = await waitForMenu();
  for (const key of ["Space", "Enter"]) {
    if (menu) return menu;
    await browser.keys("Escape");
    await browser.keys(key);
    menu = await waitForMenu();
  }
  if (menu) return menu;
  throw new Error("Chat model menu did not open");
}

async function chooseLunaMax() {
  const modelMenu = await displayed('[aria-label="Model settings for this message"]');
  await browser.waitUntil(async () => await modelMenu.isEnabled(), {
    timeout: 90_000,
    interval: 150,
    timeoutMsg: `Timed out waiting for the Chat model catalog\n${await bodyText()}`,
  });
  await openModelMenu(modelMenu);
  await clickMenuRadio("GPT-5.6 Luna");
  await clickMenuRadio("max");
  await dismissOverlays();
  const composer = await displayed('textarea[aria-label="Message Guru"]');
  await composer.click();
  assert.match(await modelMenu.getText(), /GPT-5\.6 Luna/i);
  assert.match(await modelMenu.getText(), /max/i);
}

async function assertLunaMaxSelection(context) {
  const modelMenu = await displayed('[aria-label="Model settings for this message"]');
  const label = await modelMenu.getText();
  assert.match(label, /GPT-5\.6 Luna/i, `${context}: Luna selection changed`);
  assert.match(label, /max/i, `${context}: max thinking selection changed`);
}

async function composerCheckbox(labelText) {
  const checkbox = await browser.waitUntil(
    async () => {
      for (const label of await browser.$$("label.composer-checkbox")) {
        if (!(await label.isDisplayed())) continue;
        if (!(await label.getText()).includes(labelText)) continue;
        const input = await label.$('input[type="checkbox"]');
        if (await input.isExisting()) return input;
      }
      return false;
    },
    {
      timeout: 15_000,
      interval: 150,
      timeoutMsg: `Timed out waiting for ${labelText} checkbox\n${await bodyText()}`,
    },
  );
  return checkbox;
}

async function setComposerCheckbox(labelText, selected) {
  const checkbox = await composerCheckbox(labelText);
  if ((await checkbox.isSelected()) !== selected) {
    await checkbox.click();
  }
  assert.equal(await checkbox.isSelected(), selected, `${labelText} should be ${selected}`);
  return checkbox;
}

async function sendPrompt(text) {
  const composer = await displayed('textarea[aria-label="Message Guru"]');
  await composer.setValue(text);
  await (await displayed('button[aria-label="Send"]')).click();
}

async function assertVisibleStreamedAssistantDelta(
  caseName,
  timeout = FIRST_VISIBLE_ASSISTANT_DELTA_TIMEOUT_MS,
) {
  await browser.waitUntil(
    async () => {
      for (const article of await browser.$$(
        "article.message.assistant.streaming",
      )) {
        if (!(await isVisible(article))) continue;
        try {
          for (const projection of await article.$$(
            ".chat-progress-commentary-live, .message-content",
          )) {
            if (!(await isVisible(projection))) continue;
            const text = (await projection.getText()).replace(/\s+/gu, " ").trim();
            if (!text || /^Starting response…?$/u.test(text)) continue;
            acceptanceObservations.streamedAssistantDelta = {
              caseName,
              visible: true,
              characters: text.length,
            };
            return true;
          }
        } catch {
          // The streaming card may be replaced while React receives another delta.
        }
      }
      return false;
    },
    {
      timeout,
      interval: 150,
      timeoutMsg: `Timed out waiting for a visible nonempty streamed assistant delta (${caseName})\n${await bodyText()}`,
    },
  );
  assert.equal(
    acceptanceObservations.streamedAssistantDelta?.visible,
    true,
    `${caseName}: streamed assistant delta was not observed`,
  );
}

async function waitUntilIdle(timeout = 30_000) {
  await browser.waitUntil(
    async () => {
      for (const button of await browser.$$('button[aria-label="Stop response"]')) {
        if (await isVisible(button)) return false;
      }
      for (const article of await browser.$$("article.message.assistant.streaming")) {
        if (await isVisible(article)) return false;
      }
      return true;
    },
    {
      timeout,
      interval: 150,
      timeoutMsg: `Chat did not go idle\n${await bodyText()}`,
    },
  );
}

async function visibleWorkProgresses() {
  const progress = [];
  for (const candidate of await browser.$$('[aria-label="Work progress"]')) {
    if (await isVisible(candidate)) progress.push(candidate);
  }
  return progress;
}

async function latestWorkProgressAfter(previousCount, timeout = 60_000) {
  await browser.waitUntil(
    async () => (await visibleWorkProgresses()).length > previousCount,
    {
      timeout,
      interval: 150,
      timeoutMsg: `Timed out waiting for the current response Work progress\n${await bodyText()}`,
    },
  );
  const progress = await visibleWorkProgresses();
  const latest = progress.at(-1);
  assert.ok(latest, "Current response Work progress disappeared");
  return latest;
}

async function assertNoVisibleAlerts(caseName) {
  for (const alert of await browser.$$('[role="alert"]')) {
    if (await isVisible(alert)) {
      throw new Error(`${caseName}: visible alert: ${await alert.getText()}`);
    }
  }
}

async function assertNoAssistantError(caseName) {
  for (const article of await browser.$$("article.message.assistant.error")) {
    if (await isVisible(article)) {
      throw new Error(`${caseName}: assistant error: ${await article.getText()}`);
    }
  }
}

async function assertTurnHealthy(caseName) {
  await assertNoAssistantError(caseName);
  await assertNoVisibleAlerts(caseName);
  const visible = await bodyText();
  const compactionFailure = textContainsCompactionEnableFailure(visible);
  if (compactionFailure) {
    throw new Error(`${caseName}: ${compactionFailure}`);
  }
  const progress = await collectWorkProgress(browser);
  const failedCompact = failedCompactRows(progress);
  if (failedCompact.length) {
    throw new Error(
      `${caseName}: compact row failed: ${JSON.stringify(failedCompact)}`,
    );
  }
  return progress;
}

async function latestProgressRows(caseName) {
  const [latest] = await collectWorkProgress(browser, { latestOnly: true });
  assert.ok(latest, `${caseName}: latest assistant response omitted Work progress`);
  assert.ok(Array.isArray(latest.rows), `${caseName}: Work progress rows are invalid`);
  return latest.rows;
}

function exactlyOneSucceededProgressRow(rows, expected, caseName) {
  const matches = rows.filter(
    (row) =>
      row.category === expected.category &&
      row.operation === expected.operation &&
      row.action === expected.action,
  );
  assert.equal(
    matches.length,
    1,
    `${caseName}: expected exactly one ${expected.action} row, got ${JSON.stringify(matches)}`,
  );
  assert.equal(
    matches[0].status,
    "succeeded",
    `${caseName}: ${expected.action} did not succeed: ${JSON.stringify(matches[0])}`,
  );
  if (expected.target != null) {
    assert.equal(
      matches[0].target,
      expected.target,
      `${caseName}: ${expected.action} targeted the wrong component: ${JSON.stringify(matches[0])}`,
    );
  }
  return matches[0];
}

function assertOnlyNonSystemProgressRows(rows, expected, caseName) {
  const nonSystemRows = rows.filter((row) => row.category !== "system");
  assert.equal(
    nonSystemRows.length,
    expected.length,
    `${caseName}: unexpected non-system Work progress rows: ${JSON.stringify(nonSystemRows)}`,
  );
  for (const item of expected) {
    exactlyOneSucceededProgressRow(nonSystemRows, item, caseName);
  }
}

const observedTokens = [];

function observeToken(token) {
  observedTokens.push(token);
  console.log(token);
}

async function dismissOverlays() {
  for (let attempt = 0; attempt < 3; attempt += 1) {
    if (!(await visibleModelMenu())) return;
    await browser.keys("Escape");
    try {
      await browser.waitUntil(async () => !(await visibleModelMenu()), {
        timeout: 1_500,
        interval: 100,
      });
      return;
    } catch {
      // The native WebKit driver can retain a stale focus target after a
      // RadioItem selection. Retry through the trigger only after testing the
      // user-visible Escape path above.
    }
    const trigger = await browser.$('[aria-label="Model settings for this message"]');
    if (
      (await isVisible(trigger)) &&
      (await trigger.getAttribute("aria-expanded")) === "true"
    ) {
      await trigger.click();
    }
    try {
      await browser.waitUntil(async () => !(await visibleModelMenu()), {
        timeout: 1_500,
        interval: 100,
      });
      return;
    } catch {
      // Use a stable outside control before retrying Escape. This mirrors a
      // normal user click without relying on a WebDriver script scroll.
      const chatTab = await browser.$("#main-tab-chat");
      if (await isVisible(chatTab)) await chatTab.click();
      await browser.keys("Escape");
    }
  }
  if (await visibleModelMenu()) {
    throw new Error("Chat model menu did not close after Escape");
  }
}

async function persistWorkProgress(label, extra = {}) {
  const progress = await collectWorkProgress(browser);
  await dismissOverlays();
  const compaction = compactionObservation(progress);
  const dump = {
    label,
    phase,
    compaction,
    observedTokens,
    acceptanceObservations,
    missingHarnessActions: missingHarnessActions(progress),
    progress,
    ...extra,
  };
  const json = `${JSON.stringify(dump, null, 2)}\n`;
  const filename =
    label === "verify" ? "work-progress-verify.json" : "work-progress.json";
  await writeFile(resolve(artifactRoot, filename), json);
  const evidenceDir = process.env.GURUTERMINAL_LIVE_CHAT_EVIDENCE_DIR;
  if (evidenceDir) {
    await mkdir(evidenceDir, { recursive: true });
    await writeFile(resolve(evidenceDir, filename), json);
  }
  console.log(compaction.note);
  return dump;
}

async function latestCompleteAssistant(previousCount, timeout = 180_000, caseName = "turn") {
  await browser.waitUntil(
    async () => {
      for (const article of await browser.$$("article.message.assistant.error")) {
        if (await isVisible(article)) {
          throw new Error(`${caseName}: assistant error: ${await article.getText()}`);
        }
      }
      return (await browser.$$("article.message.assistant.complete")).length > previousCount;
    },
    {
      timeout,
      interval: 500,
      timeoutMsg: `Timed out waiting for a new complete assistant message (${caseName})\n${await bodyText()}`,
    },
  );
  const articles = await browser.$$("article.message.assistant.complete");
  return articles.at(-1);
}

async function screenshot(name) {
  await dismissOverlays();
  await browser.saveScreenshot(resolve(artifactRoot, `native-live-chat-${name}.png`));
}

async function runCompletedTurn() {
  const useMemoryToggle = await composerCheckbox("Use memory");
  const updateMemoryToggle = await composerCheckbox("Update memory");
  assert.equal(await useMemoryToggle.isSelected(), true, "Use memory should default on");
  assert.equal(await updateMemoryToggle.isSelected(), true, "Update memory should default on");
  await updateMemoryToggle.click();
  assert.equal(await updateMemoryToggle.isSelected(), false);
  assert.equal(
    await useMemoryToggle.isSelected(),
    true,
    "Update memory must not change Use memory",
  );
  await useMemoryToggle.click();
  assert.equal(await useMemoryToggle.isSelected(), false);
  assert.equal(await updateMemoryToggle.isSelected(), false);
  await updateMemoryToggle.click();
  assert.equal(await updateMemoryToggle.isSelected(), true);
  assert.equal(
    await useMemoryToggle.isSelected(),
    false,
    "Use memory must remain independent",
  );
  await setComposerCheckbox("Use memory", true);
  await setComposerCheckbox("Update memory", true);

  const previousComplete = (await browser.$$("article.message.assistant.complete")).length;
  await sendPrompt(
    "For a native end-to-end streaming test, write exactly eight short numbered lines explaining why a UI must render a response progressively. Do not call any tool. End the eighth line with this exact token: LUNA-NATIVE-E2E-COMPLETE",
  );
  await displayed('button[aria-label="Stop response"]', 15_000);
  await assertVisibleStreamedAssistantDelta("complete-token");
  await screenshot("streaming");

  const assistant = await latestCompleteAssistant(previousComplete, 180_000, "complete-token");
  await waitForText(assistant, "LUNA-NATIVE-E2E-COMPLETE", 30_000);
  observeToken("LUNA-NATIVE-E2E-COMPLETE");
  await waitForText(assistant, "Luna", 15_000);
  await waitForText(assistant, "max", 15_000);
  await waitUntilIdle();
  await assertTurnHealthy("complete-token");
}

async function runFinanceCoreTurn() {
  await setComposerCheckbox("Use memory", false);
  await setComposerCheckbox("Update memory", false);

  const previousComplete = (await browser.$$("article.message.assistant.complete")).length;
  await sendPrompt(
    "Native acceptance test. Do not use Memory, web, compute, or any connector. Follow this exact sequence: first call capability_search exactly once with query \"finance calculations\"; then, from its result, call capability_load exactly once for the Finance calculations component; then call finance_calculate exactly once with operation \"percentage_change\" and arguments start \"80\", end \"100\", and precision 2. Do not call any other tool. After all three tools succeed, reply exactly: LUNA-FINANCE-CORE-25-PERCENT-E2E: 25 percent",
  );
  await displayed('button[aria-label="Stop response"]', 15_000);
  await displayed('[aria-label="Work progress"]', 60_000);

  const assistant = await latestCompleteAssistant(previousComplete, 180_000, "finance-core");
  await waitForText(assistant, financeResultToken, 30_000);
  assert.match(
    (await assistant.getText()).replace(/\s+/gu, " "),
    /\b25(?:\.0+)?\s*(?:percent\b|%)/iu,
    "finance-core: the deterministic percentage result was not visible in the assistant answer",
  );
  observeToken(financeResultToken);

  const rows = await latestProgressRows("finance-core");
  const searched = exactlyOneSucceededProgressRow(
    rows,
    {
      category: "capability",
      operation: "search",
      action: "Searched tools",
    },
    "finance-core",
  );
  assert.match(
    searched.target ?? "",
    /finance/i,
    `finance-core: capability discovery query was not finance-specific: ${JSON.stringify(searched)}`,
  );
  exactlyOneSucceededProgressRow(
    rows,
    {
      category: "capability",
      operation: "read",
      action: "Opened a tool",
      target: "guruterminal.finance-core/calculations",
    },
    "finance-core",
  );
  exactlyOneSucceededProgressRow(
    rows,
    {
      category: "finance",
      operation: "calculate",
      action: "Calculated financial data",
      target: "percentage_change",
    },
    "finance-core",
  );
  acceptanceObservations.financeCore = {
    calculation: "percentage_change",
    visibleResult: "25 percent",
    capabilityDiscovery: true,
    capabilityLoad: true,
  };

  await waitUntilIdle();
  await assertTurnHealthy("finance-core");
  await setComposerCheckbox("Use memory", true);
  await setComposerCheckbox("Update memory", true);
  await screenshot("finance-core");
}

async function runWorldBankMacroTurn() {
  await setComposerCheckbox("Use memory", false);
  await setComposerCheckbox("Update memory", false);

  const previousComplete = (await browser.$$("article.message.assistant.complete")).length;
  await sendPrompt(
    "Native acceptance test. Do not use Memory, web, compute, artifacts, or any connector other than the named World Bank component. Follow this exact sequence: first call capability_load exactly once with id \"guruterminal.finance-providers/macro-data\"; then call finance_macro_data exactly once with provider \"world-bank.indicators\", economy \"USA\", indicator \"NY.GDP.MKTP.CD\", start_year 2020, and end_year 2021. Do not call any other tool. After both tools succeed, reply exactly: LUNA-WORLD-BANK-MACRO-E2E",
  );
  await displayed('button[aria-label="Stop response"]', 15_000);
  await displayed('[aria-label="Work progress"]', 60_000);

  const assistant = await latestCompleteAssistant(
    previousComplete,
    240_000,
    "world-bank-macro",
  );
  await waitForText(assistant, worldBankResultToken, 30_000);
  observeToken(worldBankResultToken);

  const rows = await latestProgressRows("world-bank-macro");
  assertOnlyNonSystemProgressRows(
    rows,
    [
      {
        category: "capability",
        operation: "read",
        action: "Opened a tool",
        target: "guruterminal.finance-providers/macro-data",
      },
      {
        category: "finance",
        operation: "read",
        action: "Read macro data",
        target: "world-bank.indicators · USA · NY.GDP.MKTP.CD · 2020 · 2021",
      },
    ],
    "world-bank-macro",
  );
  acceptanceObservations.worldBankMacro = {
    component: "guruterminal.finance-providers/macro-data",
    provider: "world-bank.indicators",
    economy: "USA",
    indicator: "NY.GDP.MKTP.CD",
    years: [2020, 2021],
  };

  await waitUntilIdle();
  await assertTurnHealthy("world-bank-macro");
  await setComposerCheckbox("Use memory", true);
  await setComposerCheckbox("Update memory", true);
  await screenshot("world-bank-macro");
}

async function runOpenbbKeylessTurn() {
  await setComposerCheckbox("Use memory", false);
  await setComposerCheckbox("Update memory", false);

  const previousComplete = (await browser.$$("article.message.assistant.complete")).length;
  await sendPrompt(
    `Native acceptance test. Do not read or search Memory, web, compute, or artifacts, and do not use any connector other than the named OpenBB component. Follow this exact sequence: first call capability_load exactly once with id "mcp/openbb"; then call mcp__openbb__activate_tools exactly once with tool_names ["equity_price_quote"]; then call mcp__openbb__equity_price_quote exactly once with symbol "AAPL" and provider "yfinance"; then use that quote response's exact result_ref to call evidence_create exactly once with title "${openbbEvidenceTitle}", summary "Receipt for the keyless OpenBB AAPL quote.", as_of "2026-08-24", and one claim whose text says the selected provider was yfinance and whose sole citation uses that result_ref at JSON Pointer "/structuredContent/provider". Do not call any other tool. After all four tools succeed, reply exactly: ${openbbResultToken}: <the exact quote result_ref>`,
  );
  await displayed('button[aria-label="Stop response"]', 15_000);
  await displayed('[aria-label="Work progress"]', 60_000);

  const assistant = await latestCompleteAssistant(
    previousComplete,
    240_000,
    "openbb-keyless",
  );
  const response = await assistant.$(".message-content");
  assert.ok(
    await response.isExisting(),
    "openbb-keyless: assistant response body is missing",
  );
  const assistantText = (await response.getText()).replace(/\s+/gu, " ").trim();
  const resultRefMatch = assistantText.match(
    new RegExp(`^${openbbResultToken}: (result:[A-Za-z0-9_-]{1,128})$`),
  );
  assert.ok(
    resultRefMatch,
    `openbb-keyless: assistant did not return exactly one delivered quote result_ref: ${assistantText}`,
  );
  const [, resultRef] = resultRefMatch;
  observeToken(openbbResultToken);
  observeToken(resultRef);
  await waitForText(assistant, "Sources saved", 30_000);
  const evidenceSummary = await assistant.$("details.memory-update-footer.applied > summary");
  assert.ok(
    await evidenceSummary.isExisting(),
    "openbb-keyless: delivered quote receipt was not finalized as Evidence",
  );
  if ((await evidenceSummary.getAttribute("aria-expanded")) !== "true") {
    await evidenceSummary.click();
  }
  await waitForText(assistant, openbbEvidenceTitle, 30_000);

  const rows = await latestProgressRows("openbb-keyless");
  const nonSystemRows = rows.filter((row) => row.category !== "system");
  assert.equal(
    nonSystemRows.length,
    4,
    `openbb-keyless: unexpected non-system Work progress rows: ${JSON.stringify(nonSystemRows)}`,
  );
  exactlyOneSucceededProgressRow(
    nonSystemRows,
    {
      category: "capability",
      operation: "read",
      action: "Opened a tool",
      target: "mcp/openbb",
    },
    "openbb-keyless",
  );
  exactlyOneSucceededProgressRow(
    nonSystemRows,
    {
      category: "memory",
      operation: "publish",
      action: "Created evidence",
      target: openbbEvidenceTitle,
    },
    "openbb-keyless",
  );
  const marketRows = nonSystemRows.filter(
    (row) =>
      row.category === "finance" &&
      row.operation === "read" &&
      row.action === "Read market data",
  );
  assert.equal(
    marketRows.length,
    2,
    `openbb-keyless: expected activation and quote Work progress rows, got ${JSON.stringify(marketRows)}`,
  );
  const activationRows = marketRows.filter((row) => row.target === null);
  assert.equal(
    activationRows.length,
    1,
    `openbb-keyless: expected one OpenBB activation row, got ${JSON.stringify(marketRows)}`,
  );
  assert.equal(
    activationRows[0].status,
    "succeeded",
    `openbb-keyless: OpenBB activation failed: ${JSON.stringify(activationRows[0])}`,
  );
  const quoteRows = marketRows.filter((row) => row.target === "AAPL · yfinance");
  assert.equal(
    quoteRows.length,
    1,
    `openbb-keyless: expected one AAPL/yfinance quote row, got ${JSON.stringify(marketRows)}`,
  );
  assert.equal(
    quoteRows[0].status,
    "succeeded",
    `openbb-keyless: OpenBB AAPL quote failed: ${JSON.stringify(quoteRows[0])}`,
  );
  await waitUntilIdle();
  await assertTurnHealthy("openbb-keyless");
  const citation = await readOpenbbEvidenceCitation(resultRef);
  acceptanceObservations.openbbKeyless = {
    component: "mcp/openbb",
    activationTool: "mcp__openbb__activate_tools",
    quoteTool: "mcp__openbb__equity_price_quote",
    provider: "yfinance",
    symbol: "AAPL",
    resultRef,
    evidenceTitle: openbbEvidenceTitle,
    evidenceCitation: citation,
  };

  await setComposerCheckbox("Use memory", true);
  await setComposerCheckbox("Update memory", true);
  await screenshot("openbb-keyless");
}

async function readOpenbbEvidenceCitation(resultRef) {
  await dismissOverlays();
  await clickButton("Memory");
  const library = await displayed("#main-panel-library");
  const search = await displayed('input[placeholder="Search memory"]');
  await search.setValue(openbbEvidenceTitle);

  const matchingResults = async () => {
    const matches = [];
    for (const button of await library.$$("button[data-library-result]")) {
      if (!(await isVisible(button))) continue;
      if ((await button.getAttribute("data-kind")) !== "evidence") continue;
      const label = await button.getAttribute("aria-label");
      if (label?.startsWith(`Open ${openbbEvidenceTitle} (Evidence,`)) {
        matches.push(button);
      }
    }
    return matches;
  };
  await browser.waitUntil(
    async () => (await matchingResults()).length === 1,
    {
      timeout: 30_000,
      interval: 150,
      timeoutMsg: `openbb-keyless: stored Evidence did not appear as one exact visible result\n${await bodyText()}`,
    },
  );
  const [evidenceResult] = await matchingResults();
  assert.ok(evidenceResult, "openbb-keyless: stored Evidence search result disappeared");
  await evidenceResult.click();
  await waitForText(library, openbbEvidenceTitle, 30_000);
  await clickButton("Raw", library);

  const raw = await displayed("pre.raw-markdown", 30_000);
  const markdown = (await raw.getText()).replace(/\r\n?/gu, "\n");
  const citations = [
    ...markdown.matchAll(
      /^## Claim \d+ · Citation \d+\n\n- Result: `([^`\n]+)`\n- JSON Pointer: `([^`\n]+)`\n- Selection: (?:exact value|exact excerpt)$/gmu,
    ),
  ];
  assert.equal(
    citations.length,
    1,
    `openbb-keyless: stored Evidence must contain one citation, got ${citations.length}`,
  );
  const [, storedResultRef, pointer] = citations[0];
  assert.equal(
    storedResultRef,
    resultRef,
    "openbb-keyless: stored Evidence citation must retain the OpenBB quote result_ref",
  );
  assert.equal(
    pointer,
    "/structuredContent/provider",
    "openbb-keyless: stored Evidence citation must select the OpenBB provider pointer",
  );

  await clickButton("Chat");
  await displayed('textarea[aria-label="Message Guru"]');
  return { resultRef: storedResultRef, pointer };
}

async function runWebResearchTurn() {
  await setComposerCheckbox("Use memory", false);
  await setComposerCheckbox("Update memory", false);

  const previousComplete = (await browser.$$("article.message.assistant.complete")).length;
  await sendPrompt(
    "Native acceptance test. Do not use Memory, finance, compute, or artifacts. Follow this exact sequence: first call capability_load exactly once with id \"community.web-research/research\"; then call web_search exactly once with query \"IANA example domain\" and limit 2; then call web_fetch exactly once with url \"https://example.com/\". The fixed fetch URL is deliberate and independent of the search result; do not replace it with a source_id. Do not call any other tool. After all three tools succeed, reply exactly: LUNA-WEB-RESEARCH-E2E",
  );
  await displayed('button[aria-label="Stop response"]', 15_000);
  await displayed('[aria-label="Work progress"]', 60_000);

  const assistant = await latestCompleteAssistant(
    previousComplete,
    240_000,
    "web-research",
  );
  await waitForText(assistant, webResearchResultToken, 30_000);
  observeToken(webResearchResultToken);

  const rows = await latestProgressRows("web-research");
  assertOnlyNonSystemProgressRows(
    rows,
    [
      {
        category: "capability",
        operation: "read",
        action: "Opened a tool",
        target: "community.web-research/research",
      },
      {
        category: "web",
        operation: "search",
        action: "Searched the web",
        target: "IANA example domain",
      },
      {
        category: "web",
        operation: "read",
        action: "Read a web source",
        target: "example.com · example.com",
      },
    ],
    "web-research",
  );
  acceptanceObservations.webResearch = {
    component: "community.web-research/research",
    searchQuery: "IANA example domain",
    fetchedUrl: "https://example.com/",
  };

  await waitUntilIdle();
  await assertTurnHealthy("web-research");
  await setComposerCheckbox("Use memory", true);
  await setComposerCheckbox("Update memory", true);
  await screenshot("web-research");
}

async function runArtifactTurn() {
  const turnStartedAt = Date.now();
  const previousComplete = (await browser.$$("article.message.assistant.complete")).length;
  const previousProgressCount = (await visibleWorkProgresses()).length;
  await sendPrompt(
    `For a native end-to-end test, call artifact_publish exactly once and no other tool. Use mode "create", title "${artifactTitle}", and payload kind "markdown" with schema "guruterminal-markdown/1". Its Markdown must have this exact structure: first line "# ${artifactToken}"; one blank line; then the single sentence "A short sentence." After the tool succeeds, reply briefly.`,
  );
  await displayed('button[aria-label="Stop response"]', 15_000);

  const progress = await latestWorkProgressAfter(previousProgressCount);
  assert.equal(
    await progress.$("button.chat-progress-toggle").getAttribute("aria-expanded"),
    "true",
  );
  await browser.waitUntil(
    async () => {
      for (const article of await browser.$$("article.message.assistant.error")) {
        if (await isVisible(article)) {
          throw new Error("artifact: the Chat turn failed before publishing its artifact");
        }
      }
      return (await progress.getText()).includes("Published a Chat artifact");
    },
    {
      timeout: FIRST_TOOL_PROGRESS_TIMEOUT_MS,
      interval: 500,
      timeoutMsg: "artifact: timed out waiting for publish progress",
    },
  );
  acceptanceObservations.artifact = {
    firstToolProgressMs: Date.now() - turnStartedAt,
  };

  await browser.waitUntil(
    async () =>
      (await progress.$("button.chat-progress-toggle").getAttribute("aria-expanded")) ===
      "false",
    {
      timeout: 180_000,
      interval: 200,
      timeoutMsg: `Artifact turn did not settle and collapse its progress\n${await bodyText()}`,
    },
  );

  await latestCompleteAssistant(previousComplete, 30_000, "artifact");
  const openViewer = await browser.$('[aria-label="Chat workspace panel"]');
  if (!(await openViewer.isDisplayed())) {
    await (
      await displayed(
        `button[aria-label="Open document ${artifactTitle}"]`,
        180_000,
      )
    ).click();
  }
  const viewer = await displayed('[aria-label="Chat workspace panel"]');
  await waitForText(viewer, artifactTitle);
  await waitForText(viewer, artifactToken);

  await clickButton("Source", viewer);
  await waitForText(await displayed('[aria-label="Chat workspace panel"]'), `# ${artifactToken}`);
  await clickButton("Preview");
  await waitForText(await displayed('[aria-label="Chat workspace panel"]'), artifactToken);
  await browser.keys("Escape");
  await waitUntilIdle();
  await assertTurnHealthy("artifact");
  await screenshot("artifact");
}

async function runEvidenceChartDecisionTurn() {
  await setComposerCheckbox("Update memory", false);
  assert.equal(await (await composerCheckbox("Use memory")).isSelected(), true);

  const previousComplete = (await browser.$$("article.message.assistant.complete")).length;
  await sendPrompt(
    `Run this exact native lineage test sequentially and do not use any other tool. ` +
      `First call compute_run with JavaScript source ` +
      '`function main() { return { rows: [{ date: "2026-08-20", value: 10 }, { date: "2026-08-21", value: 20 }, { date: "2026-08-22", value: 30 }], claim: "The final value is 30." }; }`. ' +
      `Second, use that returned result_ref to call evidence_create once with title "${lineageEvidenceTitle}", ` +
      `summary "Exact deterministic lineage test data.", as_of "2026-08-24", and one claim citing pointer "/data/rows/2/value". ` +
      `Third, call chart_publish once with mode=create, title "${lineageChartTitle}", a from_result dataset using the same result_ref, ` +
      `rows_pointer "/data/rows", date/date at pointer "/date", value/number at pointer "/value", and an analytic line view with x=date and y=[value]. ` +
      `Fourth, call decision_submit once with stance=neutral, horizon="test-only", probability=0.5, ` +
      `thesis="${lineageDecisionTitle}", evidence_ids containing only the evidence_id returned above, ` +
      `uses_ids=[], risks=["Test data only"], and invalidation_conditions=["Any tool failure"]. ` +
      `After all four tools succeed, reply with exactly ${lineageToken}.`,
  );
  await displayed('button[aria-label="Stop response"]', 15_000);

  const assistant = await latestCompleteAssistant(previousComplete, 240_000, "lineage");
  await waitForText(assistant, lineageToken, 30_000);
  observeToken(lineageToken);
  await waitForText(assistant, "Judgment saved", 30_000);
  observeToken("Judgment saved");
  const summary = await assistant.$("summary");
  if (await summary.isDisplayed()) {
    await summary.click();
  }
  await waitForText(assistant, lineageEvidenceTitle, 15_000);

  const progressDump = await collectWorkProgress(browser);
  const latestProgress = progressDump.at(-1);
  assert.ok(latestProgress, "Lineage turn did not expose Work progress");
  const progressItems = await browser.$$('[aria-label="Work progress"]');
  const progress = progressItems.at(-1);
  assert.ok(progress, "Lineage turn did not expose Work progress");
  await expandProgress(progress);
  const actions = new Set((latestProgress.rows ?? []).map((row) => row.action));
  for (const label of [
    "Ran a sandboxed calculation",
    "Created evidence",
    "Published a chart",
    "Submitted a decision",
  ]) {
    assert.ok(
      actions.has(label),
      `Lineage Work progress omitted ${label}: ${JSON.stringify(latestProgress.rows)}`,
    );
  }

  const openChart = await displayed(
    `button[aria-label="Open chart ${lineageChartTitle}"]`,
    30_000,
  );
  await openChart.click();
  const viewer = await displayed('[aria-label="Chat workspace panel"]');
  await waitForText(viewer, lineageChartTitle);
  await waitForText(viewer, "3 rows");
  await displayed('[aria-label="Analytic chart"]');
  await browser.keys("Escape");
  await waitUntilIdle();
  await assertTurnHealthy("lineage");
  await screenshot("evidence-chart-decision");
}

async function expandMemorySummary(article) {
  const learnedSummary = await article.$("summary");
  if (await learnedSummary.isDisplayed()) {
    await learnedSummary.click();
  }
}

async function runLearnThenCiteTurns() {
  await setComposerCheckbox("Use memory", true);
  await setComposerCheckbox("Update memory", true);

  const composer = await displayed('textarea[aria-label="Message Guru"]');
  let previousComplete = (await browser.$$("article.message.assistant.complete")).length;
  await composer.setValue(
    `$wiki Teach this Guru a standing method titled "${wikiTitle}": when a named customer has committed in-house packaging, do not treat foundry CoWoS quotes as the binding constraint. The Wiki frontmatter must include id, title, summary, and as_of as RFC3339 with seconds and timezone like 2026-08-24T00:00:00Z. Include Scope, Assumptions, Counterexamples, Limits, and Invalidation conditions.`,
  );
  assert.equal(await (await composerCheckbox("Use memory")).isSelected(), true);
  assert.equal(await (await composerCheckbox("Update memory")).isSelected(), true);
  await (await displayed('button[aria-label="Send"]')).click();
  const learned = await latestCompleteAssistant(previousComplete, 240_000, "wiki-teach");
  await waitForText(learned, "Guru learned", 30_000);
  observeToken("Guru learned");
  await expandMemorySummary(learned);
  await waitForText(learned, wikiTitle, 15_000);
  observeToken(wikiTitle);
  await waitUntilIdle();
  await assertTurnHealthy("wiki-teach");
  await screenshot("wiki");

  previousComplete = (await browser.$$("article.message.assistant.complete")).length;
  await sendPrompt(
    `A customer just disclosed committed in-house packaging. Apply the stored method and cite its title.`,
  );
  const applied = await latestCompleteAssistant(previousComplete, 240_000, "wiki-cite");
  await waitForText(applied, wikiTitle, 30_000);
  await waitUntilIdle();
  await assertTurnHealthy("wiki-cite");
}

async function runLensTeachThenApplyTurns() {
  await setComposerCheckbox("Use memory", true);
  await setComposerCheckbox("Update memory", true);

  const composer = await displayed('textarea[aria-label="Message Guru"]');
  let previousComplete = (await browser.$$("article.message.assistant.complete")).length;
  await composer.setValue(
    `$lens Teach this Guru an interpretive lens titled "${lensTitle}": when a company is early in a capacity-addition cycle, treat margin expansion as a lagging confirmation, not a reason to add. The Lens frontmatter must include id, title, summary, and as_of as RFC3339 with seconds and timezone like 2026-08-24T00:00:00Z. Include Scope, Assumptions, Counterexamples, Limits, and Invalidation conditions.`,
  );
  assert.equal(await (await composerCheckbox("Use memory")).isSelected(), true);
  assert.equal(await (await composerCheckbox("Update memory")).isSelected(), true);
  await (await displayed('button[aria-label="Send"]')).click();
  const learned = await latestCompleteAssistant(previousComplete, 240_000, "lens-teach");
  await waitForText(learned, "Guru learned", 30_000);
  observeToken("Guru learned");
  await expandMemorySummary(learned);
  await waitForText(learned, lensTitle, 15_000);
  observeToken(lensTitle);
  await waitUntilIdle();
  await assertTurnHealthy("lens-teach");
  await screenshot("lens");

  previousComplete = (await browser.$$("article.message.assistant.complete")).length;
  await sendPrompt(
    `A semiconductor equipment vendor is announcing a large capacity expansion while margins are still rising. Apply the stored lens and cite its title.`,
  );
  const applied = await latestCompleteAssistant(previousComplete, 240_000, "lens-apply");
  await waitForText(applied, lensTitle, 30_000);
  await waitUntilIdle();
  await assertTurnHealthy("lens-apply");
}

async function assertMemoryTitles(caseName) {
  await dismissOverlays();
  await clickButton("Memory");
  const panel = await displayed("#main-panel-library");
  for (const title of [wikiTitle, lensTitle, lineageEvidenceTitle, lineageDecisionTitle]) {
    await waitForText(panel, title, 30_000);
    observeToken(title);
  }
  await screenshot("memory");
  await clickButton("Chat");
  await displayed('textarea[aria-label="Message Guru"]');
  await assertTurnHealthy(caseName);
}

async function runFollowupTurn() {
  const previousComplete = (await browser.$$("article.message.assistant.complete")).length;
  await sendPrompt(
    `For a native end-to-end follow-up after the long sequence, reply with exactly this token and nothing else: ${followupToken}`,
  );
  await displayed('button[aria-label="Stop response"]', 15_000);
  const assistant = await latestCompleteAssistant(previousComplete, 180_000, "follow-up");
  await waitForText(assistant, followupToken, 30_000);
  observeToken(followupToken);
  await waitUntilIdle();
  await assertTurnHealthy("follow-up");
  await screenshot("followup");
}

async function runAbortedTurn() {
  await browser.waitUntil(
    async () => /max/i.test(await (await displayed('[aria-label="Model settings for this message"]')).getText()),
    {
      timeout: 15_000,
      interval: 100,
      timeoutMsg: "The exact max thinking selection was not retained for the follow-up",
    },
  );
  await sendPrompt(
    "Write a detailed thirty-section investment research handbook. Each section must contain at least five paragraphs. Begin immediately.",
  );
  const stop = await displayed('button[aria-label="Stop response"]', 15_000);
  await browser.pause(500);
  await stop.click();
  const aborted = await displayed("article.message.assistant.aborted", 30_000);
  await waitForText(aborted, "Stopped", 15_000);
  observeToken("Stopped");
  await waitUntilIdle(15_000);
  await assertNoVisibleAlerts("abort");
  await assertNoAssistantError("abort");
  const compactionFailure = textContainsCompactionEnableFailure(await bodyText());
  if (compactionFailure) {
    throw new Error(`abort: ${compactionFailure}`);
  }
  const progress = await collectWorkProgress(browser);
  const failedCompact = failedCompactRows(progress);
  if (failedCompact.length) {
    throw new Error(`abort: compact row failed: ${JSON.stringify(failedCompact)}`);
  }
  await screenshot("abort");
}

async function verifyRestartedChat() {
  const main = await displayed("main");
  await waitForText(main, "LUNA-NATIVE-E2E-COMPLETE", 30_000);
  await waitForText(main, followupToken, 30_000);
  await displayed("article.message.assistant.complete");
  const aborted = await displayed("article.message.assistant.aborted");
  await waitForText(aborted, "Stopped");
  await waitForText(main, "Luna");
  await waitForText(main, "max");
  assert.match(
    await (await displayed('[aria-label="Model settings for this message"]')).getText(),
    /luna/i,
  );
  assert.match(
    await (await displayed('[aria-label="Model settings for this message"]')).getText(),
    /medium/i,
  );

  const openArtifact = await displayed(
    `button[aria-label="Open document ${artifactTitle}"]`,
  );
  await openArtifact.click();
  const viewer = await displayed('[aria-label="Chat workspace panel"]');
  await waitForText(viewer, artifactTitle);
  await waitForText(viewer, artifactToken);
  await browser.keys("Escape");

  const openLineageChart = await displayed(
    `button[aria-label="Open chart ${lineageChartTitle}"]`,
  );
  await openLineageChart.click();
  const lineageViewer = await displayed('[aria-label="Chat workspace panel"]');
  await waitForText(lineageViewer, lineageChartTitle);
  await waitForText(lineageViewer, "3 rows");
  let lineageMessage = null;
  for (const article of await browser.$$("article.message.assistant.complete")) {
    if ((await article.getText()).includes("Judgment saved")) {
      lineageMessage = article;
      break;
    }
  }
  assert.ok(lineageMessage, "Restarted Chat omitted the sealed lineage judgment");
  const lineageSummary = await lineageMessage.$("summary");
  if ((await lineageSummary.getAttribute("aria-expanded")) !== "true") {
    await lineageSummary.click();
  }
  await waitForText(lineageMessage, lineageEvidenceTitle);

  await assertNoAssistantError("verify");
  await assertNoVisibleAlerts("verify");
  const compactionFailure = textContainsCompactionEnableFailure(await bodyText());
  if (compactionFailure) {
    throw new Error(`verify: ${compactionFailure}`);
  }
  const progress = await collectWorkProgress(browser);
  const failedCompact = failedCompactRows(progress);
  if (failedCompact.length) {
    throw new Error(`verify: compact row failed: ${JSON.stringify(failedCompact)}`);
  }

  await dismissOverlays();
  await clickButton("Memory");
  const panel = await displayed("#main-panel-library");
  for (const title of [wikiTitle, lensTitle, lineageEvidenceTitle, lineageDecisionTitle]) {
    await waitForText(panel, title, 30_000);
    observeToken(title);
  }
}

async function runRestartedMemoryReuseTurn() {
  await dismissOverlays();
  await clickButton("Chat");
  await (
    await displayed('button[aria-label="New session for Luna Native Agent"]')
  ).click();

  const main = await displayed("main");
  await waitForText(main, "New chat", 15_000);
  await waitForText(main, "Ask Luna Native Agent", 15_000);
  await browser.waitUntil(
    async () => (await browser.$$("article.message")).length === 0,
    {
      timeout: 15_000,
      interval: 150,
      timeoutMsg: `Restarted Memory reuse did not begin from a new empty Chat\n${await bodyText()}`,
    },
  );
  await chooseLunaMax();
  await setComposerCheckbox("Use memory", true);
  await setComposerCheckbox("Update memory", false);

  const previousComplete = (await browser.$$("article.message.assistant.complete")).length;
  await sendPrompt(
    `This is a new Chat with no earlier transcript to rely on. Use only Memory tools. First call memory_search exactly once to find the durable guidance relevant to a customer with committed in-house packaging and a semiconductor equipment vendor entering a capacity-addition cycle while margins are rising. From its results, exact-read both relevant Wiki and Lens records with memory_read. Do not use any other tool. Then apply both records in a concise answer, cite both stored titles, and end with exactly ${restartedMemoryToken}.`,
  );
  await displayed('button[aria-label="Stop response"]', 15_000);
  await displayed('[aria-label="Work progress"]', 60_000);

  const assistant = await latestCompleteAssistant(
    previousComplete,
    240_000,
    "restart-memory-reuse",
  );
  for (const title of [wikiTitle, lensTitle]) {
    await waitForText(assistant, title, 30_000);
    const used = await assistant.$(`button[aria-label="Used note: ${title}"]`);
    assert.ok(
      (await used.isExisting()) && (await isVisible(used)),
      `restart-memory-reuse: exact Memory read was not visibly attributed to ${title}`,
    );
  }
  await waitForText(assistant, restartedMemoryToken, 30_000);
  observeToken(restartedMemoryToken);

  const rows = await latestProgressRows("restart-memory-reuse");
  const searches = rows.filter(
    (row) =>
      row.category === "memory" &&
      row.operation === "search" &&
      row.action === "Searched Memory",
  );
  assert.equal(
    searches.length,
    1,
    `restart-memory-reuse: expected one semantic Memory search, got ${JSON.stringify(searches)}`,
  );
  assert.equal(
    searches[0].status,
    "succeeded",
    `restart-memory-reuse: Memory search failed: ${JSON.stringify(searches[0])}`,
  );
  const reads = rows.filter(
    (row) =>
      row.category === "memory" &&
      row.operation === "read" &&
      row.action === "Read Memory",
  );
  assert.ok(
    reads.length >= 2,
    `restart-memory-reuse: expected exact reads for Wiki and Lens, got ${JSON.stringify(reads)}`,
  );
  assert.ok(
    reads.every((row) => row.status === "succeeded"),
    `restart-memory-reuse: Memory read failed: ${JSON.stringify(reads)}`,
  );
  const nonMemoryRows = rows.filter((row) => row.category !== "memory");
  assert.deepEqual(
    nonMemoryRows,
    [],
    `restart-memory-reuse: new Chat used non-Memory tools: ${JSON.stringify(nonMemoryRows)}`,
  );
  acceptanceObservations.restartedMemoryReuse = {
    newSession: true,
    semanticSearch: true,
    exactReads: reads.length,
    citedTitles: [wikiTitle, lensTitle],
  };

  await waitUntilIdle();
  await assertTurnHealthy("restart-memory-reuse");
  await screenshot("restart-memory-reuse");
}

try {
  if (
    [
      "smoke",
      "run",
      "artifact",
      "artifact-memory",
      "artifact-after-finance",
      "artifact-after-finance-warm",
      "artifact-after-finance-no-history",
    ].includes(phase)
  ) {
    await createLiveAgent();
    await connectExistingPiProfile();
    await clickButton("Chat");
    await (await displayed('button[aria-label="New session for Luna Native Agent"]')).click();
    await chooseLunaMax();
    await runCompletedTurn();
  }

  if (phase === "smoke") {
    await persistWorkProgress("smoke");
    await screenshot("smoke");
    console.log("Native Luna max Chat smoke passed.");
  } else if (phase === "artifact") {
    await setComposerCheckbox("Use memory", false);
    await setComposerCheckbox("Update memory", false);
    await runArtifactTurn();
    await persistWorkProgress("artifact");
    console.log("Native Luna max Chat artifact smoke passed.");
  } else if (phase === "artifact-memory") {
    await runArtifactTurn();
    await persistWorkProgress("artifact-memory");
    console.log("Native Luna max Chat artifact-memory smoke passed.");
  } else if (phase === "artifact-after-finance") {
    await runFinanceCoreTurn();
    await runArtifactTurn();
    await persistWorkProgress("artifact-after-finance");
    console.log("Native Luna max Chat artifact-after-finance smoke passed.");
  } else if (phase === "artifact-after-finance-warm") {
    await runFinanceCoreTurn();
    await assertLunaMaxSelection("after Finance");
    await setComposerCheckbox("Use memory", false);
    await setComposerCheckbox("Update memory", false);
    await assertLunaMaxSelection("before warm Artifact");
    await runArtifactTurn();
    await persistWorkProgress("artifact-after-finance-warm");
    console.log("Native Luna max Chat artifact-after-finance-warm smoke passed.");
  } else if (phase === "artifact-after-finance-no-history") {
    await runFinanceCoreTurn();
    await runArtifactTurn();
    await persistWorkProgress("artifact-after-finance-no-history");
    console.log("Native Luna max Chat artifact-after-finance-no-history smoke passed.");
  } else if (phase === "run") {
    await runFinanceCoreTurn();
    await assertLunaMaxSelection("after Finance");
    await setComposerCheckbox("Use memory", false);
    await setComposerCheckbox("Update memory", false);
    await assertLunaMaxSelection("before warm Artifact");
    await runArtifactTurn();
    await runWorldBankMacroTurn();
    await runOpenbbKeylessTurn();
    await runWebResearchTurn();
    await runEvidenceChartDecisionTurn();
    await runLearnThenCiteTurns();
    await runLensTeachThenApplyTurns();
    await assertMemoryTitles("memory-view");
    await runFollowupTurn();
    await runAbortedTurn();
    const dump = await persistWorkProgress("run");
    const missing = dump.missingHarnessActions;
    assert.deepEqual(
      missing,
      [],
      `Work progress omitted harness actions: ${missing.join(", ")}`,
    );
    await screenshot("complete");
    console.log("Native Luna max Chat run passed.");
  } else {
    await verifyRestartedChat();
    await screenshot("restarted");
    await runRestartedMemoryReuseTurn();
    await persistWorkProgress("verify");
    console.log("Native Luna max Chat restart passed.");
  }
} catch (error) {
  try {
    await persistWorkProgress(`${phase}-failure`);
  } catch {
    // Keep the original failure.
  }
  await screenshot(`${phase}-failure`);
  throw error;
} finally {
  await browser.deleteSession();
}
