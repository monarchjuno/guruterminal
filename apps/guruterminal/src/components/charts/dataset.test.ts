import { describe, expect, it } from "vitest";
import type { ChartDataset } from "../../types";
import { datasetRows, numericValue, timestampValue } from "./dataset";

const dataset: ChartDataset = {
  id: "dataset-1",
  columns: [
    { id: "date", label: "Date", kind: "date" },
    { id: "close", label: "Close", kind: "number" },
  ],
  rows: [["2026-08-12", "101.25"]],
  lineage: { kind: "agent_authored", upstream_receipts: [] },
  digest: "d".repeat(64),
};

describe("chart dataset adapters", () => {
  it("maps the native column matrix without changing its stored values", () => {
    expect(datasetRows(dataset)).toEqual([
      { date: "2026-08-12", close: "101.25" },
    ]);
  });

  it("normalizes provider decimal strings and second timestamps for KLineChart", () => {
    expect(numericValue("101.25")).toBe(101.25);
    expect(numericValue("not-a-number")).toBeUndefined();
    expect(timestampValue(1_786_492_800)).toBe(1_786_492_800_000);
    expect(timestampValue("2026-08-12")).toBe(Date.parse("2026-08-12"));
    expect(timestampValue(-2_208_988_800_000)).toBeUndefined();
    expect(timestampValue(10 ** 20)).toBeUndefined();
  });
});
