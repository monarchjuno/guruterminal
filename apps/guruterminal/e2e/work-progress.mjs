/**
 * Work-progress dump shared by the agent driver and isolated live Chat suite.
 * Rows come from the shipped DOM (`data-progress-*`), not a reimplemented oracle.
 */

export async function isVisible(element) {
  try {
    return await element.isDisplayed();
  } catch {
    return false;
  }
}

async function revealAndExpand(button) {
  if (!(await button.isExisting())) return;
  try {
    await button.scrollIntoView();
  } catch {
    // Attribute dump still works if the row is already in the DOM.
  }
  if (!(await isVisible(button))) return;
  if ((await button.getAttribute("aria-expanded")) !== "true") {
    await button.click();
  }
}

export async function expandProgress(section) {
  await revealAndExpand(await section.$("button.chat-progress-toggle"));
  for (const group of await section.$$("button.chat-progress-group-toggle")) {
    await revealAndExpand(group);
  }
}

export async function progressRows(section) {
  const rows = [];
  for (const row of await section.$$(".chat-progress-row")) {
    const target = await row.$(".chat-progress-target");
    rows.push({
      category: await row.getAttribute("data-progress-category"),
      operation: await row.getAttribute("data-progress-operation"),
      status: await row.getAttribute("data-progress-status"),
      action: (await (await row.$(".chat-progress-action")).getText()).trim(),
      target: (await target.isExisting())
        ? (await target.getText()).trim() || null
        : null,
    });
  }
  return rows;
}

export async function collectWorkProgress(
  browser,
  { latestOnly = false, requireVisible = false } = {},
) {
  const sections = latestOnly
    ? await (async () => {
        const messages = await browser.$$(
          [
            "article.message.assistant.complete",
            "article.message.assistant.aborted",
            "article.message.assistant.error",
          ].join(", "),
        );
        const latest = messages.at(-1);
        if (!latest) return [];
        const section = await latest.$('[aria-label="Work progress"]');
        return (await section.isExisting()) ? [section] : [];
      })()
    : await browser.$$('[aria-label="Work progress"]');
  const progress = [];
  for (const section of sections) {
    if (requireVisible && !(await isVisible(section))) continue;
    try {
      await section.scrollIntoView();
    } catch {
      // Off-screen progress still has attributes once expanded.
    }
    await expandProgress(section);
    const heading = await section.$(".chat-progress-heading");
    progress.push({
      heading: (await heading.isExisting())
        ? (await heading.getText()).trim()
        : null,
      rows: await progressRows(section),
    });
  }
  return progress;
}

export function compactProgressRows(progress) {
  return progress.flatMap((section) =>
    (section.rows ?? []).filter(
      (row) =>
        row.operation === "compact" ||
        row.action === "Compacting conversation context",
    ),
  );
}

export function failedCompactRows(progress) {
  return compactProgressRows(progress).filter((row) => row.status === "failed");
}

export function compactionObservation(progress) {
  const rows = compactProgressRows(progress);
  if (!rows.length) {
    return {
      observed: false,
      note: "compaction: none observed",
      rows: [],
    };
  }
  return {
    observed: true,
    note: `compaction: ${rows.length} row(s)`,
    rows,
  };
}

export const COMPACTION_ENABLE_FAILURE_TOKENS = [
  "session compaction configuration",
  "Pi could not enable session compaction",
  "Pi session is not idle or compactable",
];

export function textContainsCompactionEnableFailure(text) {
  return (
    COMPACTION_ENABLE_FAILURE_TOKENS.find((token) => text.includes(token)) ??
    null
  );
}

export const HARNESS_PROGRESS_ACTIONS = [
  "Ran a sandboxed calculation",
  "Published a Chat artifact",
  "Created evidence",
  "Published a chart",
  "Submitted a decision",
];

export function missingHarnessActions(progress) {
  const seen = new Set(
    progress.flatMap((section) =>
      (section.rows ?? []).map((row) => row.action),
    ),
  );
  return HARNESS_PROGRESS_ACTIONS.filter((action) => !seen.has(action));
}
