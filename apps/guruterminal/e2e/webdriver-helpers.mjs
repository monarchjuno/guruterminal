/**
 * Shared WebdriverIO helpers for the native E2E scripts. Each script creates
 * one helper set per browser session with its own timeout profile.
 */
export function createWebdriverHelpers(browser, options = {}) {
  const {
    defaultTimeout = 10_000,
    bodyTextLimit = 16_000,
    interval = 100,
  } = options;

  const bodyText = async () =>
    (await browser.$("body").getText()).slice(0, bodyTextLimit);

  async function displayed(selector, timeout = defaultTimeout) {
    await browser.waitUntil(
      async () => {
        for (const element of await browser.$$(selector)) {
          if (await element.isDisplayed()) return true;
        }
        return false;
      },
      {
        timeout,
        interval,
        timeoutMsg: `Timed out waiting for visible ${selector}\n${await bodyText()}`,
      },
    );
    for (const element of await browser.$$(selector)) {
      if (await element.isDisplayed()) return element;
    }
    throw new Error(`Visible element disappeared: ${selector}`);
  }

  async function childWithText(root, selector, text, timeout = defaultTimeout) {
    await browser.waitUntil(
      async () => {
        for (const element of await root.$$(selector)) {
          if (
            (await element.isDisplayed()) &&
            (await element.getText()).trim() === text
          ) {
            return true;
          }
        }
        return false;
      },
      {
        timeout,
        interval,
        timeoutMsg: `Timed out waiting for ${selector} with text “${text}”\n${await bodyText()}`,
      },
    );
    for (const element of await root.$$(selector)) {
      if (
        (await element.isDisplayed()) &&
        (await element.getText()).trim() === text
      ) {
        return element;
      }
    }
    throw new Error(`Element disappeared: ${selector} with text “${text}”`);
  }

  async function clickButton(text, root = browser) {
    const button = await childWithText(root, "button", text);
    await button.click();
    return button;
  }

  async function waitForText(root, text, timeout = defaultTimeout) {
    await browser.waitUntil(async () => (await root.getText()).includes(text), {
      timeout,
      interval,
      timeoutMsg: `Timed out waiting for text “${text}”\n${await bodyText()}`,
    });
  }

  async function waitForTextGone(root, text, timeout = defaultTimeout) {
    await browser.waitUntil(
      async () => !(await root.getText()).includes(text),
      {
        timeout,
        interval,
        timeoutMsg: `Timed out waiting for text to disappear: “${text}”`,
      },
    );
  }

  async function waitForAppReady(timeout = 90_000) {
    await browser.waitUntil(
      async () => !(await bodyText()).includes("Opening"),
      {
        timeout,
        interval,
        timeoutMsg: `Timed out waiting for Guru Terminal to finish opening\n${await bodyText()}`,
      },
    );
  }

  return {
    bodyText,
    displayed,
    childWithText,
    clickButton,
    waitForText,
    waitForTextGone,
    waitForAppReady,
  };
}
