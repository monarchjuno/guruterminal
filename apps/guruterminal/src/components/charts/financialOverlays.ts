import { registerOverlay, type OverlayFigure, type OverlayTemplate } from "klinecharts";
import { arrow, fibonacciExtension, measure, rect } from "@klinecharts/extension";

const STOP_FILL = "rgba(244, 63, 94, 0.18)";
const TARGET_FILL = "rgba(16, 185, 129, 0.18)";
const STOP_LINE = "#f43f5e";
const TARGET_LINE = "#10b981";
const ENTRY_LINE = "#64748b";

const formatPrice = (value: number): string => {
  const digits = Math.abs(value) >= 1 ? 2 : 4;
  return value.toFixed(digits);
};

const positionOverlay = (name: "longPosition" | "shortPosition"): OverlayTemplate => ({
  name,
  totalStep: 4,
  needDefaultPointFigure: true,
  needDefaultXAxisFigure: true,
  needDefaultYAxisFigure: true,
  createPointFigures: ({ coordinates, overlay }) => {
    if (coordinates.length < 3) return [];
    const [entry, stop, target] = coordinates;
    const entryValue = overlay.points[0]?.value;
    const stopValue = overlay.points[1]?.value;
    const targetValue = overlay.points[2]?.value;
    if (
      entryValue === undefined ||
      stopValue === undefined ||
      targetValue === undefined ||
      !Number.isFinite(entryValue) ||
      !Number.isFinite(stopValue) ||
      !Number.isFinite(targetValue)
    ) {
      return [];
    }
    const risk = Math.abs(entryValue - stopValue);
    const reward = Math.abs(targetValue - entryValue);
    const ratio = risk === 0 ? 0 : reward / risk;
    const xLeft = Math.min(entry.x, stop.x, target.x);
    const xRight = Math.max(entry.x, stop.x, target.x);
    const width = Math.max(xRight - xLeft, 8);
    const textX = xRight + 6;
    const zone = (fromY: number, toY: number, color: string): OverlayFigure => ({
      type: "rect",
      attrs: {
        x: xLeft,
        y: Math.min(fromY, toY),
        width,
        height: Math.max(Math.abs(toY - fromY), 1),
      },
      styles: { style: "fill", color },
      ignoreEvent: true,
    });
    const line = (y: number, color: string): OverlayFigure => ({
      type: "line",
      attrs: { coordinates: [{ x: xLeft, y }, { x: xRight, y }] },
      styles: { color },
    });
    const caption = (y: number, text: string): OverlayFigure => ({
      type: "text",
      ignoreEvent: true,
      attrs: { x: textX, y, text, align: "left", baseline: "middle" },
    });
    return [
      zone(entry.y, stop.y, STOP_FILL),
      zone(entry.y, target.y, TARGET_FILL),
      line(entry.y, ENTRY_LINE),
      line(stop.y, STOP_LINE),
      line(target.y, TARGET_LINE),
      caption(entry.y, `Entry ${formatPrice(entryValue)}`),
      caption(stop.y, `Stop ${formatPrice(stopValue)}`),
      caption(target.y, `Target ${formatPrice(targetValue)}  R:R ${ratio.toFixed(2)}`),
    ];
  },
});

let registered = false;

export const registerReviewedOverlays = (): void => {
  if (registered) return;
  registerOverlay(rect);
  registerOverlay(arrow);
  registerOverlay(measure);
  registerOverlay(fibonacciExtension);
  registerOverlay(positionOverlay("longPosition"));
  registerOverlay(positionOverlay("shortPosition"));
  registered = true;
};
