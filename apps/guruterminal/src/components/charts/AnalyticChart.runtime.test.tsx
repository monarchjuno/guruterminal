import { render, waitFor } from "@testing-library/react";
import type { ChartDataset, ChartDocument } from "../../types";

const { embed } = vi.hoisted(() => ({
  embed: vi.fn().mockResolvedValue({ finalize: vi.fn() }),
}));

vi.mock("vega-embed", () => ({ default: embed }));

import AnalyticChart from "./AnalyticChart";

it("runs Vega in CSP-safe interpreter mode", async () => {
  const dataset: ChartDataset = {
    id: "dataset-1",
    columns: [
      { id: "date", label: "Date", kind: "date" },
      { id: "close", label: "Close", kind: "number" },
    ],
    rows: [
      ["2026-08-01", 10],
      ["2026-08-02", 11],
    ],
    lineage: { kind: "agent_authored", upstream_receipts: [] },
    digest: "a".repeat(64),
  };
  const chart: ChartDocument = {
    dataset_id: dataset.id,
    dataset_digest: dataset.digest,
    view: {
      kind: "analytic",
      chart_type: "line",
      x: "date",
      y: ["close"],
      semantic_types: { date: "Date", close: "Price" },
    },
    studies: [],
    drawings: [],
  };

  render(<AnalyticChart chart={chart} dataset={dataset} theme="light" />);

  await waitFor(() => expect(embed).toHaveBeenCalledOnce());
  expect(embed.mock.calls[0][0]).toHaveAttribute(
    "aria-label",
    "Interactive analytic chart",
  );
  expect(embed.mock.calls[0][2]).toMatchObject({
    ast: true,
    renderer: "svg",
  });
  expect(embed.mock.calls[0][2]).not.toHaveProperty("theme");
  expect(embed.mock.calls[0][1]).not.toMatchObject({
    width: "container",
    height: "container",
  });
});
