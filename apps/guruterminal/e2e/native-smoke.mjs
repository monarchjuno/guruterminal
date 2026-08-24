import assert from "node:assert/strict";
import { mkdir, readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { remote } from "webdriverio";
import { createWebdriverHelpers } from "./webdriver-helpers.mjs";

const e2eRoot = dirname(fileURLToPath(import.meta.url));
const sessionPath = process.argv[2];
const extraArgs = process.argv.slice(3);
const full = extraArgs.includes("--full");
assert.ok(
  sessionPath && extraArgs.every((arg) => arg === "--full"),
  "usage: node native-smoke.mjs <current-session.json> [--full]",
);

const session = JSON.parse(await readFile(sessionPath, "utf8"));
const browser = await remote({
  ...session.webdriverConfig,
  capabilities: session.capabilities,
  logLevel: "error",
});

const artifactRoot = resolve(e2eRoot, "artifacts");
await mkdir(artifactRoot, { recursive: true });
const initialWindowSize = await browser.getWindowSize();

const {
  displayed,
  childWithText,
  clickButton,
  waitForText,
  waitForTextGone,
} = createWebdriverHelpers(browser, {
  defaultTimeout: 10_000,
  bodyTextLimit: 16_000,
});

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

async function selectOption(triggerSelector, optionText) {
  const trigger = await displayed(triggerSelector);
  await trigger.click();
  for (let attempt = 0; attempt < 15; attempt += 1) {
    for (const option of await browser.$$('[role="option"]')) {
      if (
        (await option.isDisplayed()) &&
        (await option.getText()).trim() === optionText
      ) {
        await option.click();
        return;
      }
    }
    await browser.pause(100);
  }

  // WebKit can display an open Radix Select without exposing its options to
  // WebDriver. Use the focused visible trigger's type-ahead, never a hidden node.
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

async function assertCurrentTab(tab) {
  const current = await displayed(`#main-tab-${tab}`);
  assert.equal(await current.getAttribute("aria-current"), "page");
  assert.equal(await current.getAttribute("data-active"), "true");
  const allCurrent = await browser.$$('[aria-label="Main views"] [aria-current="page"]');
  assert.equal(allCurrent.length, 1, "Exactly one main view must be current");
  const appearance = await browser.execute((tabId) => {
    const currentTab = document.querySelector(`#main-tab-${tabId}`);
    const inactiveTab = [
      ...document.querySelectorAll('[aria-label="Main views"] button'),
    ].find(
      (element) =>
        element !== currentTab &&
        element.getAttribute("data-active") === "false" &&
        !element.matches(":hover"),
    );
    return {
      current: currentTab
        ? window.getComputedStyle(currentTab).backgroundColor
        : null,
      inactive: inactiveTab
        ? window.getComputedStyle(inactiveTab).backgroundColor
        : null,
    };
  }, tab);
  assert.ok(appearance.current && appearance.inactive, JSON.stringify(appearance));
  assert.notEqual(
    appearance.current,
    appearance.inactive,
    `The current main view must remain visually distinct: ${JSON.stringify(appearance)}`,
  );
}

async function step(name, action) {
  process.stdout.write(`• ${name} ... `);
  await action();
  console.log("passed");
}

const capture = (name) =>
  browser.saveScreenshot(resolve(artifactRoot, `native-smoke-${name}.png`));

try {
  await step("fresh Provider onboarding and default Agent", async () => {
    const chatPanel = await displayed("main");
    await waitForText(chatPanel, "Connect a model provider");
    await clickButton("Open Settings", chatPanel);
    const settingsPanel = await displayed("#main-panel-settings");
    await waitForText(settingsPanel, "Connect provider");
    await clickButton("Back to app");
    await navigateTo("Agents");

    const agentsPanel = await displayed("#main-panel-agents");
    await assertCurrentTab("agents");
    await waitForText(agentsPanel, "My Agent");
    await waitForText(agentsPanel, "Lens");
    await waitForText(agentsPanel, "Web Research");
    await waitForText(agentsPanel, "OpenBB Platform");
    const openbbNote = await browser.execute(() => {
      const row = [...document.querySelectorAll(".agent-capability")].find(
        (candidate) =>
          candidate.querySelector("strong")?.textContent === "OpenBB Platform",
      );
      return row?.querySelector("small")?.textContent?.trim() ?? "";
    });
    assert.notEqual(
      openbbNote,
      "Set up in Marketplace",
      "OpenBB Platform has no Marketplace setup; a missing runtime must not look like a user setup gap",
    );
    if (openbbNote) {
      assert.equal(openbbNote, "Bundled runtime is missing from this build");
    }
  });

  await step("Agent skills, tools, and rename", async () => {
    const agentsPanel = await displayed("#main-panel-agents");
    const enabledSkill = await displayed(
      'button[aria-label="Research: enabled"]',
    );
    await enabledSkill.click();
    await displayed('button[aria-label="Research: disabled"]');
    await (await displayed('button[aria-label="Research: disabled"]')).click();
    await displayed('button[aria-label="Research: enabled"]');

    await clickButton("Rename", agentsPanel);
    const dialog = await displayed('[role="dialog"]');
    const rename = await displayed("#rename-guru-name");
    await rename.setValue("Release Ready Agent");
    await clickButton("Save", dialog);
    await waitForText(agentsPanel, "Release Ready Agent");
    await capture("agents");
  });

  if (full) await step("Agent layout remains readable at desktop widths", async () => {
    const auditAgentLayout = () =>
      browser.execute(() => {
        const layout = document.querySelector(".agents-layout");
        const title = document.querySelector(".agent-editor-heading h2");
        const description = document.querySelector(".agent-editor-heading p");
        const columns = layout
          ? window
              .getComputedStyle(layout)
              .gridTemplateColumns.split(/\s+/)
              .filter(Boolean).length
          : 0;
        return {
          columns,
          compactMode: window.matchMedia("(max-width: 1400px)").matches,
          viewportWidth: document.documentElement.clientWidth,
          documentWidth: document.documentElement.scrollWidth,
          titleWidth: title?.clientWidth ?? 0,
          titleScrollWidth: title?.scrollWidth ?? 0,
          descriptionWidth: description?.clientWidth ?? 0,
          descriptionScrollWidth: description?.scrollWidth ?? 0,
        };
      });
    const assertReadable = (audit) => {
      assert.equal(
        audit.columns,
        audit.compactMode ? 1 : 2,
        JSON.stringify(audit),
      );
      assert.ok(
        audit.documentWidth <= audit.viewportWidth + 1,
        `Agent layout overflows globally: ${JSON.stringify(audit)}`,
      );
      assert.ok(
        audit.titleScrollWidth <= audit.titleWidth + 1,
        `Agent name is clipped: ${JSON.stringify(audit)}`,
      );
      assert.ok(
        audit.descriptionScrollWidth <= audit.descriptionWidth + 1,
        `Agent description is clipped: ${JSON.stringify(audit)}`,
      );
    };

    // macOS may cap a native test window at the active display's work area.
    // Audit the actual CSS viewport instead of assuming WebDriver can exceed it.
    await browser.setWindowSize(1401, 700);
    assertReadable(await auditAgentLayout());
    await browser.setWindowSize(1000, 700);
    assertReadable(await auditAgentLayout());
    await browser.setWindowSize(
      initialWindowSize.width,
      initialWindowSize.height,
    );
    await browser.pause(250);
  });

  if (full) await step("Marketplace discovery, Web Research settings, and bundled capabilities", async () => {
    await navigateTo("Marketplace");
    const panel = await displayed("#main-panel-marketplace");
    await assertCurrentTab("marketplace");
    await waitForText(panel, "24 capabilities");
    const openbbCard = await browser.execute(() => {
      const heading = [...document.querySelectorAll(".marketplace-card [role='heading']")].find(
        (element) => element.textContent === "OpenBB Platform",
      );
      return heading?.closest(".marketplace-card")?.innerText ?? "";
    });
    assert.match(
      openbbCard,
      /Runtime unavailable|Ready/,
      `OpenBB Platform must expose runtime status, not a dead setup path\n${openbbCard}`,
    );
    assert.doesNotMatch(openbbCard, /Needs setup/);
    const openbbGroup = await browser.execute(() =>
      [...document.querySelectorAll(".marketplace-group h2")].some(
        (heading) => heading.textContent === "OpenBB",
      ),
    );
    assert.equal(
      openbbGroup,
      true,
      "OpenBB capabilities must be grouped under the OpenBB plugin",
    );
    const search = await displayed('input[aria-label="Search Marketplace"]');
    await search.setValue("Korean");
    await waitForText(panel, "OpenDART");
    await waitForTextGone(panel, "SEC EDGAR");
    await search.setValue("");
    await waitForText(panel, "24 capabilities");

    const installedPackageNames = [
      "Python Compute",
      "Finance Core",
      "SEC EDGAR",
      "World Bank Indicators",
      "OpenDART",
      "KRX Open API",
      "Korea Investment Open Trading API",
      "FRED",
      "Alpha Vantage",
      "OpenBB Platform",
      "Web Research",
    ];
    for (const packageName of installedPackageNames) {
      await waitForText(panel, packageName);
    }

    const clickSourceTab = async (label) => {
      const clicked = await browser.execute((name) => {
        const tab = [...document.querySelectorAll('[role="tab"]')].find((element) =>
          (element.textContent ?? "").includes(name),
        );
        if (!tab) return false;
        tab.click();
        return true;
      }, label);
      assert.equal(clicked, true, `${label} source tab must exist`);
    };
    await clickSourceTab("Community");
    await waitForText(panel, "Community is coming soon");
    await waitForText(panel, "Nothing is installed from this tab today");
    const communityInstall = await browser.execute(() =>
      [...document.querySelectorAll("button")].some((button) =>
        /install|subscribe|add plugin/i.test(button.textContent ?? ""),
      ),
    );
    assert.equal(communityInstall, false, "Community must not offer install actions");
    await clickSourceTab("Libraries");
    await waitForText(panel, "Libraries is coming soon");
    await waitForText(panel, "Wiki and Lens packs over GitHub");
    await clickSourceTab("Guru Terminal");
    await waitForText(panel, "24 capabilities");

    await (await displayed('[aria-label="Settings Web Research"]')).click();
    let dialog = await displayed('[role="dialog"]');
    await waitForText(dialog, "xAI and other providers use Exa directly");
    await selectOption('[aria-label="Search routing"]', "Exa only");
    await clickButton("Save setup", dialog);
    await waitForText(dialog, "Settings saved");
    await clickButton("Close", dialog);

    await (await displayed('[aria-label="Settings Web Research"]')).click();
    dialog = await displayed('[role="dialog"]');
    assert.equal(
      await (await displayed('[aria-label="Search routing"]')).getText(),
      "Exa only",
    );
    await selectOption('[aria-label="Search routing"]', "Automatic");
    await clickButton("Save setup", dialog);
    await waitForText(dialog, "Settings saved");
    await clickButton("Close", dialog);
    await capture("marketplace");
  });

  if (full) await step("Model, appearance, and update Settings", async () => {
    await navigateTo("Settings");
    const panel = await displayed("#main-panel-settings");
    await waitForText(panel, "Connect provider");
    const providerVisible = async () => {
      for (const trigger of await browser.$$('[aria-label="Provider"]')) {
        if (await trigger.isDisplayed()) return true;
      }
      return false;
    };
    assert.equal(
      await providerVisible(),
      false,
      "Provider must stay inside the Connect provider dialog",
    );
    await clickButton("Connect provider", panel);
    await (await displayed('[aria-label="Provider"]')).click();
    await childWithText(browser, '[role="option"]', "OpenAI with ChatGPT · Recommended");
    await childWithText(browser, '[role="option"]', "Anthropic");
    await childWithText(browser, '[role="option"]', "xAI");
    await browser.keys("Escape");
    await browser.waitUntil(
      async () => {
        for (const option of await browser.$$('[role="option"]')) {
          if (await option.isDisplayed()) return false;
        }
        return true;
      },
      { timeout: 5_000, interval: 100, timeoutMsg: "Provider options did not close" },
    );
    await browser.keys("Escape");
    await browser.waitUntil(
      async () => {
        for (const dialog of await browser.$$('[role="dialog"]')) {
          if (await dialog.isDisplayed()) return false;
        }
        return true;
      },
      {
        timeout: 5_000,
        interval: 100,
        timeoutMsg: "Connect provider dialog did not close",
      },
    );

    await clickButton("Appearance");
    const dark = await displayed('button[aria-label="Dark theme"]');
    await dark.click();
    assert.equal(await dark.getAttribute("aria-pressed"), "true");
    assert.equal(await (await displayed(".app-shell")).getAttribute("data-theme"), "dark");
    const light = await displayed('button[aria-label="Light theme"]');
    await light.click();
    assert.equal(await light.getAttribute("aria-pressed"), "true");
    await (await displayed('button[aria-label="System theme"]')).click();
    assert.equal(
      await (await displayed('button[aria-label="System theme"]')).getAttribute(
        "aria-pressed",
      ),
      "true",
    );

    await clickButton("Updates");
    await clickButton("Check for updates", panel);
    await waitForText(panel, "Automatic updates are unavailable");
    await clickButton("Back to app");
  });

  if (full) await step("Memory zero state and filters", async () => {
    await navigateTo("Memory");
    const panel = await displayed("#main-panel-library");
    await assertCurrentTab("library");
    await waitForText(panel, "0 pages.");
    await waitForText(panel, "No memories yet");
    const search = await displayed('input[placeholder="Search memory"]');
    await search.setValue("nonexistent release memory");
    await waitForText(panel, "No matching memories");
    await search.setValue("");
    await displayed('[aria-label="Filter memory by type"]');
    await capture("memory");
  });

  await step("Chat onboarding and session rename, create, and delete", async () => {
    await navigateTo("Chat");
    const panel = await displayed("main");
    await assertCurrentTab("chat");
    await waitForText(panel, "Connect a model provider");
    await childWithText(panel, "button", "Open Settings");

    await (await displayed('button[aria-label="New session for Release Ready Agent"]')).click();
    await browser.waitUntil(
      async () => (await browser.$$('button[aria-label="Rename session"]')).length === 1,
      { timeout: 10_000, interval: 100, timeoutMsg: "First Chat session was not created" },
    );
    let renameButtons = await browser.$$('button[aria-label="Rename session"]');
    assert.equal(renameButtons.length, 1);
    await renameButtons[0].moveTo();
    await renameButtons[0].click();
    let dialog = await displayed('[role="dialog"]');
    await (await displayed("#rename-thread-name")).setValue("Release smoke chat");
    await clickButton("Save", dialog);
    await waitForText(await displayed('[aria-label="Application navigation"]'), "Release smoke chat");

    await (await displayed('button[aria-label="New session for Release Ready Agent"]')).click();
    await browser.waitUntil(
      async () => (await browser.$$('button[aria-label="Rename session"]')).length === 2,
      { timeout: 10_000, interval: 100, timeoutMsg: "Second Chat session was not created" },
    );
    const deleteButtons = await browser.$$('button[aria-label="Delete session"]');
    assert.equal(deleteButtons.length, 2);
    let newChatDelete = null;
    for (const button of deleteButtons) {
      const titleId = await button.getAttribute("aria-describedby");
      if (titleId && (await browser.$(`#${titleId}`).getText()) === "New chat") {
        newChatDelete = button;
        break;
      }
    }
    assert.ok(newChatDelete, "Newly created Chat session has no delete action");
    await newChatDelete.moveTo();
    await newChatDelete.click();
    dialog = await displayed('[role="dialog"]');
    await clickButton("Delete", dialog);
    await browser.waitUntil(
      async () => (await browser.$$('button[aria-label="Rename session"]')).length === 1,
      { timeout: 10_000, interval: 100, timeoutMsg: "Chat session was not deleted" },
    );
    await waitForText(await displayed('[aria-label="Application navigation"]'), "Release smoke chat");
    await capture("chat");

    if (full) {
      renameButtons = await browser.$$('button[aria-label="Rename session"]');
      await renameButtons[0].click();
      await displayed('[role="dialog"]');
      assert.equal(
        await browser.execute(() => document.activeElement?.id ?? ""),
        "rename-thread-name",
        "Rename session should focus its name field",
      );
      await browser.keys("Escape");
      await browser.waitUntil(
        async () => (await browser.$$('[role="dialog"]')).length === 0,
        { timeout: 5_000, interval: 100, timeoutMsg: "Escape did not close Rename session" },
      );
      await waitForText(await displayed('[aria-label="Application navigation"]'), "Release smoke chat");
    }
  });

  if (full) await step("minimum window, mobile navigation, and accessible controls", async () => {
    await browser.setWindowSize(700, 560);
    await browser.pause(250);
    await (await displayed('button[data-sidebar="trigger"]')).click();
    await clickButton("Memory");
    const memoryPanel = await displayed("#main-panel-library");
    assert.equal(await memoryPanel.getAttribute("aria-hidden"), "false");
    assert.equal(await (await displayed(".context-title strong")).getText(), "Memory");

    const audit = await browser.execute(() => {
      const visible = (element) => {
        const style = window.getComputedStyle(element);
        return (
          style.display !== "none" &&
          style.visibility !== "hidden" &&
          element.getClientRects().length > 0
        );
      };
      const textFromIds = (value) =>
        (value ?? "")
          .split(/\s+/)
          .filter(Boolean)
          .map((id) => document.getElementById(id)?.textContent?.trim() ?? "")
          .filter(Boolean)
          .join(" ");
      const controls = [
        ...document.querySelectorAll(
          'button, a[href], input, textarea, select, [role="button"], [role="tab"]',
        ),
      ].filter(visible);
      const unnamed = controls
        .filter((element) => {
          const labels = element.labels
            ? [...element.labels]
                .map((label) => label.textContent?.trim() ?? "")
                .join(" ")
            : "";
          return !(
            element.getAttribute("aria-label")?.trim() ||
            textFromIds(element.getAttribute("aria-labelledby")) ||
            labels.trim() ||
            element.textContent?.trim() ||
            element.getAttribute("title")?.trim()
          );
        })
        .map((element) => element.outerHTML.slice(0, 240));
      const visibleHeadings = [...document.querySelectorAll("h1")].filter(visible);
      const main = document.querySelector("#main-panel-library");
      const rect = main?.getBoundingClientRect();
      return {
        viewportWidth: document.documentElement.clientWidth,
        documentWidth: document.documentElement.scrollWidth,
        unnamed,
        visibleH1Count: visibleHeadings.length,
        mainLeft: rect?.left ?? -1,
        mainRight: rect?.right ?? -1,
      };
    });
    assert.ok(
      audit.documentWidth <= audit.viewportWidth + 1,
      `Minimum-width layout overflows globally: ${JSON.stringify(audit)}`,
    );
    assert.deepEqual(audit.unnamed, [], "Every visible control needs an accessible name");
    assert.equal(audit.visibleH1Count, 1, "The active view should expose one visible h1");
    assert.ok(audit.mainLeft >= 0 && audit.mainRight <= audit.viewportWidth + 1);
    await capture("minimum-window");
  });

  await step("Agent deletion leaves Marketplace globally available", async () => {
    await navigateTo("Agents");
    const agentsPanel = await displayed("#main-panel-agents");
    await clickButton("Delete", agentsPanel);
    let dialog = await displayed('[role="dialog"]');
    await waitForText(dialog, "Delete agent?");
    await clickButton("Cancel", dialog);
    await waitForText(agentsPanel, "Release Ready Agent");

    await clickButton("Delete", agentsPanel);
    dialog = await displayed('[role="dialog"]');
    await clickButton("Delete agent", dialog);
    await waitForText(agentsPanel, "Create your first agent");

    await navigateTo("Marketplace");
    const marketplace = await displayed("#main-panel-marketplace");
    await waitForText(marketplace, "SEC EDGAR");
    assert.equal(await marketplace.$('[aria-label="Loading Marketplace"]').isExisting(), false);
  });

  await browser.saveScreenshot(resolve(artifactRoot, "native-release-smoke.png"));
  console.log(
    full
      ? "Guru Terminal native release smoke passed."
      : "Guru Terminal native core smoke passed.",
  );
} catch (error) {
  await browser.saveScreenshot(resolve(artifactRoot, "native-release-smoke-failure.png"));
  throw error;
} finally {
  await browser.deleteSession();
}
