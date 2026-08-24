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
assert.ok(sessionPath, "usage: node native-live-chat.mjs <current-session.json> <run|verify>");
assert.ok(["run", "verify"].includes(phase), "phase must be run or verify");

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
const wikiTitle = "WP4 cobalt-foil spare-capacity rule";
const lensTitle = "WP4 native-e2e capital-cycle lens";

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
  let visibleMatch = null;
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
          if (!(await isVisible(item))) {
            try {
              await browser.execute(
                (container, target) => {
                  const top = target.offsetTop;
                  const bottom = top + target.offsetHeight;
                  if (top < container.scrollTop) container.scrollTop = top;
                  else if (bottom > container.scrollTop + container.clientHeight) {
                    container.scrollTop = bottom - container.clientHeight;
                  }
                },
                menu,
                item,
              );
            } catch {
              continue;
            }
          }
          if (await isVisible(item)) {
            visibleMatch = item;
            return true;
          }
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
  if (!visibleMatch) {
    throw new Error(`Visible menu item disappeared: ${exactText}\n${await bodyText()}`);
  }
  await visibleMatch.click();
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
  await browser.keys("Escape");
  if ((await modelMenu.getAttribute("aria-expanded")) === "true") {
    await browser.keys("Escape");
  }
  const composer = await displayed('textarea[aria-label="Message Guru"]');
  await composer.click();
  assert.match(await modelMenu.getText(), /GPT-5\.6 Luna/i);
  assert.match(await modelMenu.getText(), /max/i);
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

const observedTokens = [];

function observeToken(token) {
  observedTokens.push(token);
  console.log(token);
}

async function dismissOverlays() {
  for (let attempt = 0; attempt < 3; attempt += 1) {
    if (!(await visibleModelMenu())) return;
    await browser.keys("Escape");
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
    "For a native end-to-end test, reply with exactly this token and nothing else: LUNA-NATIVE-E2E-COMPLETE",
  );
  await displayed('button[aria-label="Stop response"]', 15_000);
  await screenshot("streaming");

  const assistant = await latestCompleteAssistant(previousComplete, 180_000, "complete-token");
  await waitForText(assistant, "LUNA-NATIVE-E2E-COMPLETE", 30_000);
  observeToken("LUNA-NATIVE-E2E-COMPLETE");
  await waitForText(assistant, "Luna", 15_000);
  await waitForText(assistant, "max", 15_000);
  await waitUntilIdle();
  await assertTurnHealthy("complete-token");
}

async function runArtifactTurn() {
  const previousComplete = (await browser.$$("article.message.assistant.complete")).length;
  await sendPrompt(
    `For a native end-to-end test, call artifact_publish exactly once. Create a Markdown document titled "${artifactTitle}" whose Markdown contains exactly the heading "# ${artifactToken}" and one short sentence. After the tool succeeds, reply briefly.`,
  );
  await displayed('button[aria-label="Stop response"]', 15_000);

  const progress = await displayed('[aria-label="Work progress"]', 60_000);
  assert.equal(
    await progress.$("button.chat-progress-toggle").getAttribute("aria-expanded"),
    "true",
  );
  await waitForText(progress, "Published a Chat artifact", 60_000);

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
  await persistWorkProgress("verify");

  await dismissOverlays();
  await clickButton("Memory");
  const panel = await displayed("#main-panel-library");
  for (const title of [wikiTitle, lensTitle, lineageEvidenceTitle, lineageDecisionTitle]) {
    await waitForText(panel, title, 30_000);
    observeToken(title);
  }
}

try {
  if (phase === "run") {
    await createLiveAgent();
    await connectExistingPiProfile();
    await clickButton("Chat");
    await (await displayed('button[aria-label="New session for Luna Native Agent"]')).click();
    await chooseLunaMax();
    await runCompletedTurn();
    await runArtifactTurn();
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
