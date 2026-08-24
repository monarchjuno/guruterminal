import { render, screen, waitFor } from "@testing-library/react";
import type { ChartDataset, ChartDocument } from "../../types";

const { instance, init, dispose, registerOverlay } = vi.hoisted(() => {
  const instance = {
    setDataLoader: vi.fn(),
    setSymbol: vi.fn(),
    setPeriod: vi.fn(),
    setTimezone: vi.fn(),
    createIndicator: vi.fn(),
    createOverlay: vi.fn().mockReturnValue("agent-drawing"),
    removeOverlay: vi.fn(),
    resize: vi.fn(),
  };
  return {
    instance,
    init: vi.fn(() => instance),
    dispose: vi.fn(),
    registerOverlay: vi.fn(),
  };
});

vi.mock("klinecharts", () => ({ init, dispose, registerOverlay }));
vi.mock("@klinecharts/extension", () => ({
  rect: { name: "rect" },
  arrow: { name: "arrow" },
  measure: { name: "measure" },
  fibonacciExtension: { name: "fibonacciExtension" },
}));

import FinancialChart from "./FinancialChart";

const dataset: ChartDataset = {
  id: "dataset-1",
  columns: [
    { id: "date", label: "Date", kind: "date" },
    { id: "open", label: "Open", kind: "number" },
    { id: "high", label: "High", kind: "number" },
    { id: "low", label: "Low", kind: "number" },
    { id: "close", label: "Close", kind: "number" },
  ],
  rows: [
    ["2026-08-01", 100, 105, 98, 103],
    ["2026-08-02", 103, 108, 101, 106],
  ],
  lineage: { kind: "agent_authored", upstream_receipts: [] },
  digest: "a".repeat(64),
};

const chart: ChartDocument = {
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
  studies: [{ module_id: "SAR", calc_params: [] }],
  drawings: [
    {
      kind: "horizontal_line",
      points: [{ timestamp: "2026-08-01", value: 101 }],
      label: "support",
    },
  ],
};

it("renders locked agent drawings without a user drawing toolbar", async () => {
  render(<FinancialChart chart={chart} dataset={dataset} theme="light" />);

  await waitFor(() => expect(instance.createOverlay).toHaveBeenCalledWith(expect.objectContaining({
    name: "horizontalStraightLine",
    lock: true,
  })));
  expect(instance.createOverlay).toHaveBeenCalledWith(expect.objectContaining({
    name: "simpleAnnotation",
    lock: true,
    extendData: "support",
  }));
  expect(instance.createIndicator).toHaveBeenCalledWith(
    { name: "SAR", paneId: "candle_pane" },
    true,
  );
  expect(registerOverlay).toHaveBeenCalled();
  expect(screen.queryByRole("toolbar")).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Trend" })).not.toBeInTheDocument();
});
