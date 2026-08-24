import "@testing-library/jest-dom/vitest";
import { configure } from "@testing-library/react";

// Streaming-chat tests wait on real timers, so a loaded machine (CI, verify.sh
// alongside cargo builds) needs a wider async window than the 1s default.
configure({ asyncUtilTimeout: 10_000 });

Object.defineProperty(window, "matchMedia", {
  configurable: true,
  value: vi.fn().mockImplementation((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(),
  })),
});

class ResizeObserverMock implements ResizeObserver {
  disconnect = vi.fn();
  observe = vi.fn();
  unobserve = vi.fn();
}

Object.defineProperty(globalThis, "ResizeObserver", {
  configurable: true,
  value: ResizeObserverMock,
});

Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
  configurable: true,
  value: vi.fn(),
});

Object.defineProperty(HTMLCanvasElement.prototype, "getContext", {
  configurable: true,
  value: vi.fn(() => ({
    font: "",
    measureText: (text: string) => ({ width: text.length * 8 }),
  })),
});

Object.defineProperties(HTMLElement.prototype, {
  hasPointerCapture: {
    configurable: true,
    value: vi.fn().mockReturnValue(false),
  },
  releasePointerCapture: {
    configurable: true,
    value: vi.fn(),
  },
  setPointerCapture: {
    configurable: true,
    value: vi.fn(),
  },
});

Object.defineProperty(navigator, "clipboard", {
  configurable: true,
  value: { writeText: vi.fn().mockResolvedValue(undefined) },
});

Object.defineProperties(URL, {
  createObjectURL: {
    configurable: true,
    value: vi.fn(
      (blob: Blob) => `data:${blob.type || "application/octet-stream"};base64,dGVzdA==`,
    ),
  },
  revokeObjectURL: {
    configurable: true,
    value: vi.fn(),
  },
});
