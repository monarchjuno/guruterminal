import type { ChartDocument } from "../../types";
import {
  analyticTheme,
  assembleAnalyticSpec,
  fitAnalyticFrame,
  humanizeFieldLabel,
} from "./analyticAdapter";

type AnalyticView = Extract<ChartDocument["view"], { kind: "analytic" }>;

const sampleView = (
  chartType: AnalyticView["chart_type"],
  extras: Partial<AnalyticView> = {},
): AnalyticView => ({
  kind: "analytic",
  chart_type: chartType,
  x: "date",
  y: ["close"],
  semantic_types: { date: "Date", close: "Price" },
  title: "Close over time",
  ...extras,
});

const sampleRows = [
  { date: "2026-08-01", close: 2 },
  { date: "2026-08-02", close: 3 },
];

const earningsView = sampleView("line", {
  x: "period",
  y: ["revenue_krw_trillion", "operating_profit_krw_trillion"],
  semantic_types: {
    period: "Category",
    revenue_krw_trillion: "Currency",
    operating_profit_krw_trillion: "Currency",
  },
  title: "연결 매출과 영업이익",
});

const earningsRows = [
  { period: "2025 Q2", revenue_krw_trillion: 74.6, operating_profit_krw_trillion: 4.7 },
  { period: "2026 Q1", revenue_krw_trillion: 85, operating_profit_krw_trillion: 8 },
  { period: "2026 Q2", revenue_krw_trillion: 95, operating_profit_krw_trillion: 12 },
];

describe("fitAnalyticFrame", () => {
  it("uses the artifact pane instead of a cropped landscape tile", () => {
    expect(fitAnalyticFrame(520, 880)).toEqual({ width: 520, height: 880 });
  });

  it("keeps a short pane from collapsing", () => {
    expect(fitAnalyticFrame(400, 180)).toEqual({ width: 400, height: 200 });
  });

  it("falls back when the host has no size yet", () => {
    expect(fitAnalyticFrame(0, 0)).toEqual({ width: 640, height: 400 });
  });
});

describe("humanizeFieldLabel", () => {
  it("turns machine field names into legend copy", () => {
    expect(humanizeFieldLabel("revenue_krw_trillion")).toBe("Revenue");
    expect(humanizeFieldLabel("operating_profit_krw_trillion")).toBe(
      "Operating Profit",
    );
  });
});

describe("assembleAnalyticSpec", () => {
  it.each<AnalyticView["chart_type"]>(["line", "area", "bar", "scatter"])(
    "assembles a single-Y %s chart as one field instead of a static series",
    (chartType) => {
      const spec = assembleAnalyticSpec(
        sampleView(chartType),
        sampleRows,
        "light",
        400,
        320,
      );

      expect(spec.width).toBe(400);
      expect(spec.height).toBe(320);
      expect(JSON.stringify(spec)).toContain('"field":"close"');
    },
  );

  it.each<AnalyticView["chart_type"]>(["line", "area", "bar", "scatter"])(
    "pins a %s chart to the pane instead of a band step or container size",
    (chartType) => {
      const spec = assembleAnalyticSpec(
        sampleView(chartType),
        sampleRows,
        "dark",
        520,
        880,
      );

      expect(spec).toMatchObject({
        width: 520,
        height: 880,
        autosize: { type: "fit", contains: "padding" },
      });
      expect(JSON.stringify(spec)).toContain("#1b1a19");
      expect(JSON.stringify(spec)).not.toContain("Close over time");
    },
  );

  it("keeps both earnings series visible from zero with readable labels", () => {
    const spec = assembleAnalyticSpec(earningsView, earningsRows, "dark", 520, 880);
    const serialized = JSON.stringify(spec);
    const values = (spec.data as { values: Array<Record<string, unknown>> }).values;

    expect(spec.width).toBe(520);
    expect(spec.height).toBe(880);
    expect(serialized).toContain('"zero":true');
    expect(serialized).not.toContain("연결 매출과 영업이익");
    expect(serialized).not.toContain("datum.__peLast === 1");
    expect(values.map((row) => row.__flint_series_key).sort()).toEqual([
      "Operating Profit",
      "Operating Profit",
      "Operating Profit",
      "Revenue",
      "Revenue",
      "Revenue",
    ]);
  });

  it("pairs Flint houses with the app theme", () => {
    expect(analyticTheme("dark")).toBe("powerbi");
    expect(analyticTheme("light")).toBe("powerbi-light");
    expect(
      JSON.stringify(
        assembleAnalyticSpec(sampleView("bar"), sampleRows, "light", 400, 320),
      ),
    ).not.toContain("#f4f1ea");
  });
});
