import { useEffect, useRef } from "react";
import { dispose, init, type KLineData } from "klinecharts";
import type { ChartDataset, ChartDocument } from "../../types";
import { datasetRows, numericValue, timestampValue } from "./dataset";
import { drawingOverlays, indicatorConfig, periodFor } from "./financialAdapter";
import { registerReviewedOverlays } from "./financialOverlays";

registerReviewedOverlays();

type Props = {
  chart: ChartDocument;
  dataset: ChartDataset;
  theme: "light" | "dark";
};

export default function FinancialChart({ chart, dataset, theme }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container || chart.view.kind !== "financial") return;
    const view = chart.view;
    const bars = datasetRows(dataset).flatMap<KLineData>((row) => {
      const timestamp = timestampValue(row[view.time]);
      const open = numericValue(row[view.open]);
      const high = numericValue(row[view.high]);
      const low = numericValue(row[view.low]);
      const close = numericValue(row[view.close]);
      const volume = view.volume ? numericValue(row[view.volume]) : undefined;
      const turnover = view.turnover ? numericValue(row[view.turnover]) : undefined;
      return timestamp === undefined || open === undefined || high === undefined || low === undefined || close === undefined
        ? []
        : [{ timestamp, open, high, low, close, volume, turnover }];
    });
    const instance = init(container, {
      layout: {
        barSpaceLimit: { min: 2, max: 36 },
      },
      styles: {
        grid: {
          horizontal: { color: theme === "dark" ? "#253044" : "#e9eef5", style: "dashed" },
          vertical: { color: theme === "dark" ? "#253044" : "#eef2f7", style: "dashed" },
        },
        candle: {
          bar: {
            upColor: "#10b981",
            downColor: "#f43f5e",
            upBorderColor: "#10b981",
            downBorderColor: "#f43f5e",
            upWickColor: "#10b981",
            downWickColor: "#f43f5e",
          },
        },
        xAxis: { tickText: { color: theme === "dark" ? "#94a3b8" : "#64748b" } },
        yAxis: { tickText: { color: theme === "dark" ? "#94a3b8" : "#64748b" } },
      },
    });
    if (!instance) return;
    instance.setDataLoader({
      getBars: ({ type, callback }) => callback(type === "init" ? bars : [], false),
    });
    instance.setSymbol({
      ticker: view.symbol,
      pricePrecision: view.price_precision ?? 2,
      volumePrecision: 0,
    });
    instance.setPeriod(periodFor(view.interval));
    instance.setTimezone("UTC");
    for (const study of chart.studies) {
      const indicator = indicatorConfig(study);
      instance.createIndicator(indicator.value, indicator.isStack);
    }
    for (const drawing of chart.drawings) {
      for (const overlay of drawingOverlays(drawing)) {
        instance.createOverlay(overlay);
      }
    }
    instance.resize();
    const observer = new ResizeObserver(() => instance.resize());
    observer.observe(container);
    return () => {
      observer.disconnect();
      dispose(instance);
    };
  }, [chart, dataset, theme]);

  return (
    <div className="chart-financial-stage">
      <div ref={containerRef} className="chart-financial-canvas" aria-label="Interactive financial chart" />
    </div>
  );
}
