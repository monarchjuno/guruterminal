import { lazy, Suspense, useId } from "react";
import { BarChart3Icon, CandlestickChartIcon } from "lucide-react";
import type { ChartDataset, ChartDocument } from "../../types";
import { Spinner } from "../ui/spinner";
import { datasetRows } from "../charts/dataset";

const FinancialChart = lazy(() => import("../charts/FinancialChart"));
const AnalyticChart = lazy(() => import("../charts/AnalyticChart"));

type Props = {
  chart: ChartDocument;
  dataset: ChartDataset;
  theme: "light" | "dark";
};

export default function ArtifactChart({ chart, dataset, theme }: Props) {
  const { view } = chart;
  const financial = view.kind === "financial";
  const descriptionId = useId();
  const rows = financial ? datasetRows(dataset) : [];
  const first = rows[0];
  const latest = rows.at(-1);
  const barLabel = rows.length === 1 ? "bar" : "bars";
  const financialDescription = financial
    ? [
        `${view.symbol} financial price chart at ${view.interval} intervals with ${rows.length.toLocaleString("en-US")} ${barLabel}.`,
        first && latest
          ? `Range ${String(first[view.time])} to ${String(latest[view.time])}.`
          : null,
        latest ? `Latest close ${String(latest[view.close])}.` : null,
        chart.studies.length > 0
          ? `Indicators: ${chart.studies.map((study) => study.module_id).join(", ")}.`
          : null,
      ].filter(Boolean).join(" ")
    : undefined;
  return (
    <figure
      className="artifact-chart-shell"
      aria-label={financial ? "Financial chart" : "Analytic chart"}
      aria-describedby={financial ? descriptionId : undefined}
    >
      {financialDescription ? (
        <p id={descriptionId} className="sr-only">{financialDescription}</p>
      ) : null}
      <header className="chart-engine-header">
        <div className="chart-engine-identity">
          {financial ? <CandlestickChartIcon aria-hidden="true" /> : <BarChart3Icon aria-hidden="true" />}
          <span>
            <strong>{financial ? view.symbol : view.title ?? "Data analysis"}</strong>
            <small>
              {financial
                ? `${view.interval} · ${dataset.rows.length.toLocaleString("en-US")} bars`
                : `${dataset.rows.length.toLocaleString("en-US")} rows`}
            </small>
          </span>
        </div>
        {chart.studies.length > 0 ? (
          <div className="chart-study-pills" aria-label="Active indicators">
            {chart.studies.map((study) => <span key={study.module_id}>{study.module_id}</span>)}
          </div>
        ) : null}
      </header>
      <div className="chart-engine-surface">
        <Suspense fallback={<div className="artifact-panel-state" role="status"><Spinner /><span>Loading chart engine…</span></div>}>
          {view.kind === "financial" ? (
            <FinancialChart chart={chart} dataset={dataset} theme={theme} />
          ) : (
            <AnalyticChart chart={chart} dataset={dataset} theme={theme} />
          )}
        </Suspense>
      </div>
      {chart.note ? <figcaption className="artifact-chart-note">{chart.note}</figcaption> : null}
    </figure>
  );
}
