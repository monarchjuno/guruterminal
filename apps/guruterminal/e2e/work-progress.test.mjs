import assert from "node:assert/strict";
import test from "node:test";
import {
  compactionObservation,
  failedCompactRows,
  missingHarnessActions,
  textContainsCompactionEnableFailure,
} from "./work-progress.mjs";

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
