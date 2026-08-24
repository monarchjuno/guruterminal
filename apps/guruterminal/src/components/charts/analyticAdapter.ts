import type { ChartAssemblyInput } from "flint-chart/core";
import { assembleVegaLite } from "flint-chart/vegalite";
import type { ChartDocument } from "../../types";

type AnalyticView = Extract<ChartDocument["view"], { kind: "analytic" }>;
type VegaNode = Record<string, unknown>;

const flintType = (chartType: AnalyticView["chart_type"]) => ({
  line: "Line Chart",
  area: "Area Chart",
  bar: "Bar Chart",
  scatter: "Scatter Plot",
})[chartType];

export const analyticTheme = (theme: "light" | "dark") =>
  theme === "dark" ? "powerbi" : "powerbi-light";

export const fitAnalyticFrame = (width: number, height: number) => {
  const measuredWidth = Number.isFinite(width) ? Math.floor(width) : 0;
  const measuredHeight = Number.isFinite(height) ? Math.floor(height) : 0;
  return {
    width: measuredWidth > 0 ? Math.max(280, measuredWidth) : 640,
    height: measuredHeight > 0 ? Math.max(200, measuredHeight) : 400,
  };
};

export const humanizeFieldLabel = (name: string) => {
  const stripped = name
    .replace(/_(krw|usd|eur|jpy|cny)(_(trillion|billion|million|thousand|mn|bn|tn))?$/i, "")
    .replace(/_/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  if (!stripped) return name;
  return stripped.replace(/\b([a-z])/g, (letter) => letter.toUpperCase());
};

const visitVega = (node: unknown, visit: (value: VegaNode) => void) => {
  if (!node || typeof node !== "object") return;
  const value = node as VegaNode;
  visit(value);
  for (const key of ["layer", "hconcat", "vconcat", "concat"]) {
    const children = value[key];
    if (Array.isArray(children)) children.forEach((child) => visitVega(child, visit));
  }
  visitVega(value.spec, visit);
};

const uniqueXCount = (view: AnalyticView, rows: Array<Record<string, unknown>>) =>
  new Set(rows.map((row) => row[view.x]).filter((value) => value != null)).size;

const finalizeAnalyticSpec = (
  spec: VegaNode,
  frame: { width: number; height: number },
  view: AnalyticView,
  rows: Array<Record<string, unknown>>,
) => {
  spec.width = frame.width;
  spec.height = frame.height;
  spec.autosize = { type: "fit", contains: "padding" };

  const config = spec.config;
  if (config && typeof config === "object") {
    const viewConfig = (config as VegaNode).view;
    if (viewConfig && typeof viewConfig === "object") {
      delete (viewConfig as VegaNode).continuousWidth;
      delete (viewConfig as VegaNode).continuousHeight;
    }
  }

  const data = spec.data;
  if (data && typeof data === "object") {
    const values = (data as VegaNode).values;
    if (Array.isArray(values)) {
      (data as VegaNode).values = values.map((row) => {
        if (!row || typeof row !== "object") return row;
        const record = row as VegaNode;
        if (typeof record.__flint_series_key !== "string") return row;
        return {
          ...record,
          __flint_series_key: humanizeFieldLabel(record.__flint_series_key),
        };
      });
    }
  }

  const showEveryPoint =
    (view.chart_type === "line" || view.chart_type === "area") &&
    uniqueXCount(view, rows) <= 16;

  visitVega(spec, (node) => {
    const encoding = node.encoding;
    if (encoding && typeof encoding === "object") {
      const channels = encoding as VegaNode;
      const y = channels.y;
      if (y && typeof y === "object" && !Array.isArray(y)) {
        const yEnc = y as VegaNode;
        if (yEnc.type === "quantitative") {
          yEnc.scale = { ...(yEnc.scale as VegaNode | undefined), zero: true };
        }
        const axis = yEnc.axis;
        if (axis && typeof axis === "object") {
          const yAxis = axis as VegaNode;
          if (typeof yAxis.format === "string" && yAxis.format.includes("12")) {
            yAxis.format = ",.2~f";
          }
        }
      }
      const x = channels.x;
      if (x && typeof x === "object" && !Array.isArray(x)) {
        const xEnc = x as VegaNode;
        if (xEnc.axis !== null) {
          const axis =
            xEnc.axis && typeof xEnc.axis === "object"
              ? (xEnc.axis as VegaNode)
              : {};
          axis.labelAngle = 0;
          axis.labelOverlap = false;
          axis.labelLimit = 160;
          xEnc.axis = axis;
        }
      }
    }

    if (!showEveryPoint) return;
    const mark = node.mark;
    const markType =
      typeof mark === "string"
        ? mark
        : mark && typeof mark === "object"
          ? (mark as VegaNode).type
          : undefined;
    if (markType !== "point" || !Array.isArray(node.transform)) return;
    node.transform = node.transform.filter((step) => {
      if (!step || typeof step !== "object") return true;
      return (step as VegaNode).filter !== "datum.__peLast === 1";
    });
  });

  return spec;
};

export const assembleAnalyticSpec = (
  view: AnalyticView,
  rows: Array<Record<string, unknown>>,
  theme: "light" | "dark",
  width: number,
  height: number,
): Record<string, unknown> => {
  const frame = fitAnalyticFrame(width, height);
  const encodings: ChartAssemblyInput["chart_spec"]["encodings"] = {
    x: { field: view.x },
    y: view.y.length === 1
      ? { field: view.y[0] }
      : view.y.map((field) => ({ field })),
  };
  if (view.color) encodings.color = { field: view.color };
  const input: ChartAssemblyInput = {
    data: { values: rows },
    semantic_types: view.semantic_types,
    chart_spec: {
      chartType: flintType(view.chart_type),
      encodings,
      baseSize: frame,
      canvasSize: frame,
    },
    theme_spec: analyticTheme(theme),
  };
  return finalizeAnalyticSpec(assembleVegaLite(input) as VegaNode, frame, view, rows);
};
