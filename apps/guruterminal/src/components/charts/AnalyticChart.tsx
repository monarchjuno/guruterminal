import { useEffect, useMemo, useRef, useState } from "react";
import vegaEmbed, { type Result as VegaResult } from "vega-embed";
import type { ChartDataset, ChartDocument } from "../../types";
import { assembleAnalyticSpec } from "./analyticAdapter";
import { datasetRows } from "./dataset";

type Props = {
  chart: ChartDocument;
  dataset: ChartDataset;
  theme: "light" | "dark";
};

const VEGA_OPTIONS = {
  actions: false,
  ast: true,
  renderer: "svg",
} as const;

const syncVegaSize = (result: VegaResult, container: HTMLElement) => {
  const width = container.clientWidth;
  const height = container.clientHeight;
  if (width < 2 || height < 2) return;
  void result.view.width(width).height(height).resize().runAsync();
};

export default function AnalyticChart({ chart, dataset, theme }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const resultRef = useRef<VegaResult | null>(null);
  const [error, setError] = useState<string>();
  const rows = useMemo(() => datasetRows(dataset), [dataset]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container || chart.view.kind !== "analytic") return;
    let cancelled = false;
    let rendering: VegaResult | undefined;
    setError(undefined);
    try {
      const spec = assembleAnalyticSpec(
        chart.view,
        rows,
        theme,
        container.clientWidth,
        container.clientHeight,
      );
      void vegaEmbed(container, spec, VEGA_OPTIONS)
        .then((result) => {
          if (cancelled) {
            result.finalize();
            return;
          }
          rendering = result;
          resultRef.current = result;
          syncVegaSize(result, container);
        })
        .catch((cause: unknown) => {
          if (!cancelled) {
            setError(
              cause instanceof Error
                ? cause.message
                : "Could not render this analytic chart.",
            );
          }
        });
    } catch (cause) {
      setError(
        cause instanceof Error
          ? cause.message
          : "Could not compile this analytic chart.",
      );
    }
    return () => {
      cancelled = true;
      resultRef.current = null;
      rendering?.finalize();
    };
  }, [chart, rows, theme]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const observer = new ResizeObserver(() => {
      const result = resultRef.current;
      if (result) syncVegaSize(result, container);
    });
    observer.observe(container);
    return () => observer.disconnect();
  }, []);

  return (
    <div className="chart-analytic-stage">
      <div
        ref={containerRef}
        className="chart-analytic-canvas"
        aria-label="Interactive analytic chart"
      />
      {error ? (
        <div className="artifact-panel-state" role="alert">
          <strong>Chart could not be rendered</strong>
          <p>{error}</p>
        </div>
      ) : null}
    </div>
  );
}
