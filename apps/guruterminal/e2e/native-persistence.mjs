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
    await browser.saveScreenshot(resolve(artifactRoot, "native-persistence-seed.png"));
    console.log("Native persistence seed passed.");
  } else {
    const navigation = await displayed('[aria-label="Application navigation"]');
    await waitForText(navigation, "Persistent E2E Agent");
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
    await browser.saveScreenshot(resolve(artifactRoot, "native-persistence-verify.png"));
    console.log("Native persistence restart passed.");
  }
} catch (error) {
  await browser.saveScreenshot(
    resolve(artifactRoot, `native-persistence-${phase}-failure.png`),
  );
  throw error;
} finally {
  await browser.deleteSession();
}
