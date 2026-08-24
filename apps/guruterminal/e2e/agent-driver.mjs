#!/usr/bin/env node
import { readFile, mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { remote } from "webdriverio";
import { createWebdriverHelpers } from "./webdriver-helpers.mjs";
import { collectWorkProgress } from "./work-progress.mjs";

const e2eRoot = dirname(fileURLToPath(import.meta.url));
const artifactRoot = resolve(e2eRoot, "artifacts");
const sessionPath = resolve(artifactRoot, "current-session.json");
const [command = "help", ...args] = process.argv.slice(2);

const usage = `Usage:
  node apps/guruterminal/e2e/agent-driver.mjs inspect
  node apps/guruterminal/e2e/agent-driver.mjs click <selector>
  node apps/guruterminal/e2e/agent-driver.mjs click-text <visible text>
  node apps/guruterminal/e2e/agent-driver.mjs set-value <selector> <text>
  node apps/guruterminal/e2e/agent-driver.mjs select-option <selector> <exact text>
  node apps/guruterminal/e2e/agent-driver.mjs press <Enter|Escape|Tab|ArrowUp|ArrowDown>
  node apps/guruterminal/e2e/agent-driver.mjs screenshot <name>
  node apps/guruterminal/e2e/agent-driver.mjs wait-chat-idle
  node apps/guruterminal/e2e/agent-driver.mjs progress
  node apps/guruterminal/e2e/agent-driver.mjs chat-model <model> <effort>
  node apps/guruterminal/e2e/agent-driver.mjs luna-max`;

function required(value, label) {
  if (!value) throw new Error(`${label} is required\n${usage}`);
  return value;
}

async function attach() {
  let session;
  try {
    session = JSON.parse(await readFile(sessionPath, "utf8"));
  } catch {
    throw new Error(
      `No active development window. Start npm run tauri dev or apps/guruterminal/e2e/up.sh first.`,
    );
  }
  if (
    session?.webdriverConfig?.hostname !== "127.0.0.1" ||
    session?.webdriverConfig?.protocol !== "http" ||
    session?.capabilities?.browserName !== "tauri"
  ) {
    throw new Error(
      "Refusing a session that is not the launcher-owned loopback Tauri endpoint",
    );
  }
  return remote({
    ...session.webdriverConfig,
    capabilities: session.capabilities,
    logLevel: "error",
  });
}

async function visible(element) {
  try {
    return await element.isDisplayed();
  } catch {
    return false;
  }
}

async function inspect(browser) {
  const body = await browser.$("body");
  const text = (await body.getText()).slice(0, 20_000);
  const candidates = await browser.$$(
    "button, a, input, textarea, select, [role=button], [role=option], [role=menuitem], [role=menuitemradio], [role=combobox], [tabindex]",
  );
  const controls = [];
  for (const element of candidates.slice(0, 400)) {
    if (!(await visible(element))) continue;
    const tag = await element.getTagName();
    const role = await element.getAttribute("role");
    const id = await element.getAttribute("id");
    const ariaLabel = await element.getAttribute("aria-label");
    const name = await element.getAttribute("name");
    const type = await element.getAttribute("type");
    const label = ((await element.getText()) || ariaLabel || name || "")
      .trim()
      .slice(0, 300);
    const selector = id
      ? `[id=${JSON.stringify(id)}]`
      : ariaLabel
        ? `[aria-label=${JSON.stringify(ariaLabel)}]`
        : name
          ? `${tag}[name=${JSON.stringify(name)}]`
          : null;
    controls.push({ tag, role, type, label, selector });
  }
  console.log(
    JSON.stringify(
      { title: await browser.getTitle(), text, controls },
      null,
      2,
    ),
  );
}

async function displayed(browser, selector, timeout = 15_000) {
  const helpers = createWebdriverHelpers(browser, {
    defaultTimeout: timeout,
    bodyTextLimit: 20_000,
  });
  return helpers.displayed(selector, timeout);
}

async function selectExact(browser, selector, optionText) {
  const trigger = await displayed(browser, selector);
  await trigger.click();
  if ((await trigger.getAttribute("aria-expanded")) !== "true") {
    await trigger.click();
    await browser.keys("Space");
  }
  try {
    await clickVisibleMenuItem(browser, "option", optionText);
  } catch (error) {
    // WebKit can render an open Radix Select while omitting its option roles
    // from WebDriver. Keep focus on the visible trigger and use the component's
    // keyboard type-ahead; never click a hidden fallback element.
    if (!String(error).includes("Visible option not found")) throw error;
    await browser.keys(optionText);
    await browser.keys("Enter");
    await browser.waitUntil(
      async () => (await trigger.getText()).trim() === optionText,
      {
        timeout: 5_000,
        interval: 100,
        timeoutMsg: `Exact visible option was not retained: ${optionText}`,
      },
    );
  }
  return trigger;
}

async function clickVisibleMenuItem(browser, role, exactText) {
  const visibleLabels = [];
  for (let attempt = 0; attempt < 40; attempt += 1) {
    await browser.pause(100);
    let container = null;
    let items = await browser.$$(`[role="${role}"]`);
    if (role === "menuitemradio") {
      for (const menu of await browser.$$('[data-slot="dropdown-menu-content"]')) {
        if (!(await visible(menu))) continue;
        container = menu;
        items = await menu.$$(`[role="${role}"]`);
        break;
      }
      if (!container) continue;
    }
    for (const item of items) {
      const label = (await item.getText()).trim();
      if (!label) continue;
      const isVisible = await visible(item);
      if (isVisible) visibleLabels.push(label);
      if (label !== exactText) continue;
      if (isVisible) {
        await item.click();
        return;
      }
      if (container) {
        // Radix keeps clipped menu items in its roving-focus collection. Use
        // type-ahead instead of a WebKit execute script to reach one.
        await browser.keys(exactText);
        await browser.keys("Enter");
        return;
      }
    }
  }
  throw new Error(
    `Visible ${role} not found: ${exactText}; visible labels: ${visibleLabels.join(" | ") || "none"}`,
  );
}

async function openModelMenu(browser, trigger) {
  const waitForMenu = async () => {
    for (let attempt = 0; attempt < 15; attempt += 1) {
      await browser.pause(100);
      for (const menu of await browser.$$('[data-slot="dropdown-menu-content"]')) {
        if (await visible(menu)) return menu;
      }
    }
    return null;
  };
  await trigger.click();
  let menu = await waitForMenu();
  for (const key of ["Space", "Enter"]) {
    if (menu) return;
    await browser.keys("Escape");
    await browser.keys(key);
    menu = await waitForMenu();
  }
  if (!menu) throw new Error("Chat model menu did not open");
}

async function configureChatModel(browser, modelText, effortText) {
  const chatModel = '[aria-label="Model settings for this message"]';
  const chatModelControl = await browser.$(chatModel);
  if (await visible(chatModelControl)) {
    let label = (await chatModelControl.getText()).trim();
    if (!label.includes(modelText) || !label.includes(effortText)) {
      if (!(await chatModelControl.isEnabled())) {
        throw new Error(`Visible Chat model control is disabled: ${label}`);
      }
      await openModelMenu(browser, chatModelControl);
      await clickVisibleMenuItem(browser, "menuitemradio", modelText);
      await clickVisibleMenuItem(browser, "menuitemradio", effortText);
      await browser.keys("Escape");
      label = await (await displayed(browser, chatModel)).getText();
    }
    if (!label.includes(modelText) || !label.includes(effortText)) {
      throw new Error(
        `Exact Chat model selection was not retained: ${label}`,
      );
    }
    console.log(`Verified visible Chat run model: ${label}`);
    return;
  }
  throw new Error("Chat model controls are not visible");
}

async function configureLunaMax(browser) {
  const chatModel = '[aria-label="Model settings for this message"]';
  const chatModelControl = await browser.$(chatModel);
  if (await visible(chatModelControl)) {
    let label = (await chatModelControl.getText()).trim();
    if (!/GPT-5\.6 Luna/i.test(label) || !/\bmax\b/i.test(label)) {
      if (!(await chatModelControl.isEnabled())) {
        throw new Error(`Visible Chat model control is disabled: ${label}`);
      }
      await openModelMenu(browser, chatModelControl);
      await clickVisibleMenuItem(browser, "menuitemradio", "GPT-5.6 Luna");
      await clickVisibleMenuItem(browser, "menuitemradio", "max");
      await browser.keys("Escape");
      label = await (await displayed(browser, chatModel)).getText();
    }
    if (!/GPT-5\.6 Luna/i.test(label) || !/\bmax\b/i.test(label)) {
      throw new Error(
        `Exact Chat Luna/max selection was not retained: ${label}`,
      );
    }
    console.log(`Verified visible Chat run model: ${label}`);
    return;
  }
  throw new Error("Chat model controls are not visible");
}

async function dumpProgress(browser, latestOnly = false) {
  const progress = await collectWorkProgress(browser, {
    latestOnly,
    requireVisible: true,
  });
  console.log(JSON.stringify({ progress }, null, 2));
}

async function waitForChatIdle(browser) {
  await browser.waitUntil(
    async () => {
      const stop = await browser.$('button[aria-label="Stop response"]');
      if (await visible(stop)) return false;
      const live = await browser.$$(
        "article.message.assistant.streaming",
      );
      return live.length === 0;
    },
    {
      timeout: 600_000,
      interval: 500,
      timeoutMsg: "Timed out waiting for the current Chat response to settle",
    },
  );
  await dumpProgress(browser, true);
}

if (command === "help" || command === "--help" || command === "-h") {
  console.log(usage);
  process.exit(0);
}

let browser;
try {
  browser = await attach();
  if (command === "inspect") {
    await inspect(browser);
  } else if (command === "click") {
    const element = await displayed(browser, required(args[0], "selector"));
    await element.waitForEnabled({ timeout: 15_000 });
    await element.click();
    console.log("Clicked visible control.");
  } else if (command === "click-text") {
    const needle = required(args[0], "visible text");
    let clicked = false;
    const buttons = await browser.$$(
      "[role=menuitemradio], [role=menuitem], [role=option], button",
    );
    for (const button of buttons) {
      if (!(await visible(button))) continue;
      const label = ((await button.getText()) || "").trim();
      if (!label.includes(needle)) continue;
      await button.waitForEnabled({ timeout: 15_000 });
      await button.click();
      clicked = true;
      break;
    }
    if (!clicked) throw new Error(`Visible button not found: ${needle}`);
    console.log("Clicked visible control.");
  } else if (command === "set-value") {
    const element = await displayed(browser, required(args[0], "selector"));
    await element.setValue(required(args[1], "text"));
    console.log(
      "Set the visible control value (value intentionally not echoed).",
    );
  } else if (command === "select-option") {
    await selectExact(
      browser,
      required(args[0], "selector"),
      required(args[1], "option text"),
    );
    console.log(`Selected visible option: ${args[1]}`);
  } else if (command === "press") {
    const key = required(args[0], "key");
    const allowed = new Set(["Enter", "Escape", "Tab", "ArrowUp", "ArrowDown"]);
    if (!allowed.has(key)) throw new Error(`Key is not allowed: ${key}`);
    await browser.keys(key);
    console.log(`Pressed ${key}.`);
  } else if (command === "screenshot") {
    const name = required(args[0], "name");
    if (!/^[a-z0-9][a-z0-9._-]*$/i.test(name) || name.includes("..")) {
      throw new Error("Screenshot name must be a safe basename");
    }
    await mkdir(artifactRoot, { recursive: true, mode: 0o700 });
    const path = resolve(
      artifactRoot,
      name.endsWith(".png") ? name : `${name}.png`,
    );
    await browser.saveScreenshot(path);
    console.log(path);
  } else if (command === "wait-chat-idle") {
    await waitForChatIdle(browser);
  } else if (command === "progress") {
    await dumpProgress(browser, false);
  } else if (command === "chat-model") {
    await configureChatModel(
      browser,
      required(args[0], "model"),
      required(args[1], "effort"),
    );
  } else if (command === "luna-max") {
    await configureLunaMax(browser);
  } else {
    throw new Error(`Unknown command: ${command}\n${usage}`);
  }
} finally {
  if (browser) await browser.deleteSession();
}
