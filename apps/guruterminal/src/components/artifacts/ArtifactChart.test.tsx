import { render, screen } from "@testing-library/react";
import type { ChartDataset, ChartDocument } from "../../types";
import ArtifactChart from "./ArtifactChart";

vi.mock("../charts/FinancialChart", () => ({
  default: () => <div>financial-engine</div>,
}));

vi.mock("../charts/AnalyticChart", () => ({
  default: () => <div>analytic-engine</div>,
}));

const dataset: ChartDataset = {
  id: "dataset-1",
  columns: [
    { id: "date", label: "Date", kind: "date" },
    { id: "open", label: "Open", kind: "number" },
    { id: "high", label: "High", kind: "number" },
    { id: "low", label: "Low", kind: "number" },
    { id: "close", label: "Close", kind: "number" },
  ],
  rows: [["2026-08-01", 1, 3, 0, 2]],
  lineage: { kind: "agent_authored", upstream_receipts: [] },
  digest: "a".repeat(64),
};

const financial: ChartDocument = {
  dataset_id: dataset.id,
  dataset_digest: dataset.digest,
  view: {
    kind: "financial",
    symbol: "TEST",
    interval: "1d",
    time: "date",
    open: "open",
    high: "high",
    low: "low",
    close: "close",
  },
  studies: [{ module_id: "EMA", calc_params: [20] }],
  drawings: [],
};

describe("ArtifactChart", () => {
  it("routes a renderer-neutral financial document to the lazy KLine adapter", async () => {
    render(<ArtifactChart chart={financial} dataset={dataset} theme="light" />);
    expect(
      screen.getByRole("figure", { name: "Financial chart" }),
    ).toHaveAccessibleDescription(
      "TEST financial price chart at 1d intervals with 1 bar. Range 2026-08-01 to 2026-08-01. Latest close 2. Indicators: EMA.",
    );
    expect(screen.getByText("TEST")).toBeVisible();
    expect(screen.getByText("EMA")).toBeVisible();
    expect(await screen.findByText("financial-engine")).toBeVisible();
  });

  it("routes analytic documents to the Flint adapter", async () => {
    const analytic: ChartDocument = {
      dataset_id: dataset.id,
      dataset_digest: dataset.digest,
      view: {
        kind: "analytic",
        chart_type: "line",
        x: "date",
        y: ["close"],
        semantic_types: { date: "Date", close: "Price" },
        title: "Close over time",
      },
      studies: [],
      drawings: [],
    };
    render(<ArtifactChart chart={analytic} dataset={dataset} theme="dark" />);
    expect(
      screen.getByRole("figure", { name: "Analytic chart" }),
    ).toBeVisible();
    expect(screen.getByText("Close over time")).toBeVisible();
    expect(screen.queryByText(/Flint/)).not.toBeInTheDocument();
    expect(await screen.findByText("analytic-engine")).toBeVisible();
  });
});
