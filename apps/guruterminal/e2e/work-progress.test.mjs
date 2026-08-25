import assert from "node:assert/strict";
import test from "node:test";
import {
  collectWorkProgress,
  compactionObservation,
  failedCompactRows,
  missingHarnessActions,
  textContainsCompactionEnableFailure,
} from "./work-progress.mjs";

function element({
  existing = true,
  displayed = true,
  attributes = {},
  text = "",
  children = {},
  lists = {},
  onClick,
  onScroll,
} = {}) {
  return {
    async isExisting() {
      return existing;
    },
    async isDisplayed() {
      return displayed;
    },
    async getAttribute(name) {
      return attributes[name] ?? null;
    },
    async getText() {
      return text;
    },
    async $(selector) {
      return children[selector] ?? element({ existing: false });
    },
    async $$(selector) {
      return lists[selector] ?? [];
    },
    async click() {
      onClick?.();
    },
    async scrollIntoView() {
      onScroll?.();
    },
  };
}

test("compaction observation names the none-observed case explicitly", () => {
  const result = compactionObservation([
    {
      heading: "1 step",
      rows: [
        {
          operation: "publish",
          action: "Published a Chat artifact",
          status: "succeeded",
        },
      ],
    },
  ]);
  assert.equal(result.observed, false);
  assert.equal(result.note, "compaction: none observed");
});

test("failed compact rows include compact operation and compacting action", () => {
  const progress = [
    {
      heading: "Work failed",
      rows: [
        {
          operation: "compact",
          action: "Compacting conversation context",
          status: "failed",
        },
        {
          operation: "execute",
          action: "Ran a sandboxed calculation",
          status: "succeeded",
        },
      ],
    },
  ];
  assert.equal(failedCompactRows(progress).length, 1);
  assert.equal(compactionObservation(progress).observed, true);
  assert.match(compactionObservation(progress).note, /compaction: 1 row/);
});

test("missing harness actions are taken from dumped row actions", () => {
  assert.deepEqual(
    missingHarnessActions([
      {
        rows: [
          { action: "Published a Chat artifact" },
          { action: "Created evidence" },
        ],
      },
    ]),
    [
      "Ran a sandboxed calculation",
      "Published a chart",
      "Submitted a decision",
    ],
  );
});

test("compaction enable failure tokens are detected in visible text", () => {
  assert.equal(
    textContainsCompactionEnableFailure(
      "Pi session is not idle or compactable",
    ),
    "Pi session is not idle or compactable",
  );
  assert.equal(textContainsCompactionEnableFailure("Judgment saved"), null);
});

test("background progress dumps do not manually scroll every row", async () => {
  let scrolls = 0;
  let expands = 0;
  const collapsedButton = element({
    attributes: { "aria-expanded": "false" },
    onClick: () => {
      expands += 1;
    },
    onScroll: () => {
      scrolls += 1;
    },
  });
  const row = element({
    attributes: {
      "data-progress-category": "compute",
      "data-progress-operation": "execute",
      "data-progress-status": "succeeded",
    },
    children: {
      ".chat-progress-action": element({ text: "Ran a sandboxed calculation" }),
      ".chat-progress-target": element({ text: "result_123" }),
    },
  });
  const section = element({
    children: {
      "button.chat-progress-toggle": collapsedButton,
      ".chat-progress-heading": element({ text: "1 step" }),
    },
    lists: {
      "button.chat-progress-group-toggle": [],
      ".chat-progress-row": [row],
    },
    onScroll: () => {
      scrolls += 1;
    },
  });
  const browser = {
    async $$(selector) {
      assert.equal(selector, '[aria-label="Work progress"]');
      return [section];
    },
  };

  const [progress] = await collectWorkProgress(browser);

  assert.equal(scrolls, 0);
  assert.equal(expands, 1);
  assert.deepEqual(progress.rows, [
    {
      category: "compute",
      operation: "execute",
      status: "succeeded",
      action: "Ran a sandboxed calculation",
      target: "result_123",
    },
  ]);
});
