import type { ChartDataset } from "../../types";

const UNIX_SECONDS_CUTOFF = 10_000_000_000;
const JAVASCRIPT_TIMESTAMP_LIMIT_MS = 8_640_000_000_000_000;

export const datasetRows = (dataset: ChartDataset): Array<Record<string, unknown>> =>
  dataset.rows.map((row) =>
    Object.fromEntries(dataset.columns.map((column, index) => [column.id, row[index]])),
  );

export const numericValue = (value: unknown): number | undefined => {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string" && value.trim()) {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) return parsed;
  }
  return undefined;
};

export const timestampValue = (value: unknown): number | undefined => {
  if (typeof value === "number" && Number.isFinite(value)) {
    if (value < 0) return undefined;
    const timestamp = value < UNIX_SECONDS_CUTOFF ? value * 1_000 : value;
    return timestamp <= JAVASCRIPT_TIMESTAMP_LIMIT_MS ? timestamp : undefined;
  }
  if (typeof value !== "string") return undefined;
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) && Math.abs(parsed) <= JAVASCRIPT_TIMESTAMP_LIMIT_MS
    ? parsed
    : undefined;
};
