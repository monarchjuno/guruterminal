import { drawingConfig, drawingOverlays, indicatorConfig, measureLabels, periodFor } from "./financialAdapter";

describe("indicatorConfig", () => {
  it("maps only explicit bounded intervals instead of silently changing them", () => {
    expect(periodFor("15m")).toEqual({ type: "minute", span: 15 });
    expect(periodFor("1wk")).toEqual({ type: "week", span: 1 });
    expect(() => periodFor("quarterly")).toThrow(
      "Unsupported financial chart interval",
    );
    expect(() => periodFor("0d")).toThrow(
      "Unsupported financial chart interval",
    );
    expect(() => periodFor("10000m")).toThrow(
      "Unsupported financial chart interval",
    );
  });

  it("keeps empty parameters absent so KLine applies its module defaults", () => {
    expect(indicatorConfig({ module_id: "VOL", calc_params: [] })).toEqual({
      value: { name: "VOL" },
      isStack: false,
    });
  });

  it("places price studies on the candle pane and preserves explicit parameters", () => {
    expect(
      indicatorConfig({ module_id: "EMA", calc_params: [10, 20] }),
    ).toEqual({
      value: { name: "EMA", paneId: "candle_pane", calcParams: [10, 20] },
      isStack: true,
    });
    expect(indicatorConfig({ module_id: "SAR", calc_params: [] })).toEqual({
      value: { name: "SAR", paneId: "candle_pane" },
      isStack: true,
    });
    expect(indicatorConfig({ module_id: "DMI", calc_params: [14, 6] })).toEqual({
      value: { name: "DMI", calcParams: [14, 6] },
      isStack: false,
    });
  });
});

describe("drawingConfig", () => {
  it("maps persisted agent drawings to locked KLine overlays", () => {
    expect(drawingConfig({
      kind: "segment",
      points: [
        { timestamp: "2026-08-01", value: 100 },
        { timestamp: 1_786_492_800, value: 110 },
      ],
      color: "#2563EB",
      line_width: 2,
      line_style: "dashed",
    })).toEqual({
      name: "segment",
      points: [
        { timestamp: Date.parse("2026-08-01"), value: 100 },
        { timestamp: 1_786_492_800_000, value: 110 },
      ],
      lock: true,
      styles: { line: { color: "#2563EB", size: 2, style: "dashed" } },
    });
  });

  it.each([
    ["horizontal_line", "horizontalStraightLine"],
    ["vertical_line", "verticalStraightLine"],
    ["price_line", "priceLine"],
    ["fibonacci", "fibonacciLine"],
    ["parallel_line", "parallelStraightLine"],
    ["price_channel", "priceChannelLine"],
    ["annotation", "simpleAnnotation"],
    ["rectangle", "rect"],
    ["arrow", "arrow"],
    ["measure", "measure"],
    ["fibonacci_extension", "fibonacciExtension"],
    ["long_position", "longPosition"],
    ["short_position", "shortPosition"],
  ] as const)("maps %s to the %s overlay", (kind, name) => {
    expect(drawingConfig({
      kind,
      points: [{ timestamp: "2026-08-01", value: 100 }],
      label: kind === "annotation" ? "earnings" : undefined,
    }).name).toBe(name);
  });

  it("puts annotation text in extendData", () => {
    expect(drawingConfig({
      kind: "annotation",
      points: [{ timestamp: "2026-08-01", value: 100 }],
      label: "earnings",
    })).toMatchObject({
      name: "simpleAnnotation",
      lock: true,
      extendData: "earnings",
    });
  });

  it("computes measure labels from the two points", () => {
    const overlay = drawingConfig({
      kind: "measure",
      points: [
        { timestamp: "2026-08-01", value: 100 },
        { timestamp: "2026-08-02", value: 110 },
      ],
    });
    expect(overlay.name).toBe("measure");
    expect(overlay.extendData).toBe(measureLabels);
    expect(measureLabels(overlay.points ?? [])).toEqual(["+10.00", "+10.00%", "1d"]);
  });
});

describe("drawingOverlays", () => {
  it("adds a locked annotation when a geometric drawing has a label", () => {
    const overlays = drawingOverlays({
      kind: "segment",
      points: [
        { timestamp: "2026-08-01", value: 100 },
        { timestamp: "2026-08-02", value: 110 },
      ],
      label: "breakout",
    });
    expect(overlays).toHaveLength(2);
    expect(overlays[0]).toMatchObject({ name: "segment", lock: true });
    expect(overlays[1]).toEqual({
      name: "simpleAnnotation",
      points: [{ timestamp: Date.parse("2026-08-02"), value: 110 }],
      lock: true,
      extendData: "breakout",
    });
  });

  it("does not duplicate annotation drawings", () => {
    expect(drawingOverlays({
      kind: "annotation",
      points: [{ timestamp: "2026-08-01", value: 100 }],
      label: "earnings",
    })).toHaveLength(1);
  });
});
