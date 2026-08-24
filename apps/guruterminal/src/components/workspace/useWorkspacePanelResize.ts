import { useEffect, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import type { WorkspacePlacement } from "../../chat/workspace";

export type { WorkspacePlacement } from "../../chat/workspace";

export const workspaceOverlayMedia = "(max-width: 760px)";

export const workspacePanelLimits = {
  side: { min: 360, max: 1280, primaryMin: 420 },
  bottom: { min: 260, max: 720, primaryMin: 240 },
} as const;

type Options = {
  placement: WorkspacePlacement;
  width: number;
  height: number;
  onWidthChange: (width: number) => void;
  onHeightChange: (height: number) => void;
};

export function useWorkspacePanelResize({
  placement,
  width,
  height,
  onWidthChange,
  onHeightChange,
}: Options) {
  const [resizing, setResizing] = useState(false);
  const cleanupRef = useRef<(() => void) | null>(null);

  useEffect(
    () => () => {
      cleanupRef.current?.();
    },
    [],
  );

  const resizeBounds = (stage: HTMLElement | null) => {
    const limits = workspacePanelLimits[placement];
    const rect = stage?.getBoundingClientRect();
    const available = placement === "side" ? rect?.width : rect?.height;
    const max = available && available > 0
      ? Math.max(limits.min, Math.min(limits.max, available - limits.primaryMin))
      : limits.max;
    return { min: limits.min, max };
  };

  const setPanelSize = (next: number, stage: HTMLElement | null) => {
    const { min, max } = resizeBounds(stage);
    const clamped = Math.round(Math.max(min, Math.min(max, next)));
    if (placement === "side") onWidthChange(clamped);
    else onHeightChange(clamped);
  };

  const startResize = (event: PointerEvent<HTMLDivElement>) => {
    if (window.matchMedia(workspaceOverlayMedia).matches) return;
    cleanupRef.current?.();
    event.preventDefault();
    const handle = event.currentTarget;
    const stage = handle.closest(".app-stage") as HTMLElement | null;
    const stageRect = stage?.getBoundingClientRect();
    if (!stageRect) return;
    const ownerWindow = handle.ownerDocument.defaultView ?? window;
    const body = handle.ownerDocument.body;
    const pointerId = event.pointerId;
    const previousCursor = body.style.cursor;
    const previousUserSelect = body.style.userSelect;
    let resizeFrame: number | null = null;
    let pendingSize: number | null = null;

    try {
      handle.setPointerCapture(pointerId);
    } catch {
      // Synthetic pointer events and older WebViews may not register an active pointer.
      // Window-level listeners below still keep the drag bounded and deterministic.
    }
    body.style.cursor = placement === "side" ? "col-resize" : "row-resize";
    body.style.userSelect = "none";
    setResizing(true);

    const commitPendingSize = () => {
      resizeFrame = null;
      if (pendingSize === null) return;
      const next = pendingSize;
      pendingSize = null;
      setPanelSize(next, stage);
    };
    const move = (pointer: globalThis.PointerEvent) => {
      const next = placement === "side"
        ? stageRect.right - pointer.clientX
        : stageRect.bottom - pointer.clientY;
      if (!Number.isFinite(next)) return;
      pendingSize = next;
      if (resizeFrame === null) {
        resizeFrame = ownerWindow.requestAnimationFrame(commitPendingSize);
      }
    };
    const cleanup = () => {
      if (resizeFrame !== null) ownerWindow.cancelAnimationFrame(resizeFrame);
      resizeFrame = null;
      pendingSize = null;
      ownerWindow.removeEventListener("pointermove", move);
      ownerWindow.removeEventListener("pointerup", stop);
      ownerWindow.removeEventListener("pointercancel", stop);
      if (handle.hasPointerCapture(pointerId)) handle.releasePointerCapture(pointerId);
      body.style.cursor = previousCursor;
      body.style.userSelect = previousUserSelect;
      cleanupRef.current = null;
    };
    const stop = () => {
      if (resizeFrame !== null) ownerWindow.cancelAnimationFrame(resizeFrame);
      commitPendingSize();
      cleanup();
      setResizing(false);
    };

    cleanupRef.current = cleanup;
    ownerWindow.addEventListener("pointermove", move);
    ownerWindow.addEventListener("pointerup", stop, { once: true });
    ownerWindow.addEventListener("pointercancel", stop, { once: true });
  };

  const resizeWithKeyboard = (event: KeyboardEvent<HTMLDivElement>) => {
    const stage = event.currentTarget.closest(".app-stage") as HTMLElement | null;
    const current = placement === "side" ? width : height;
    const step = event.shiftKey ? 48 : 16;
    const direction = placement === "side"
      ? { ArrowLeft: step, ArrowRight: -step }
      : { ArrowUp: step, ArrowDown: -step };
    const delta = direction[event.key as keyof typeof direction];
    if (delta !== undefined) {
      event.preventDefault();
      setPanelSize(current + delta, stage);
    } else if (event.key === "Home" || event.key === "End") {
      event.preventDefault();
      const bounds = resizeBounds(stage);
      setPanelSize(event.key === "Home" ? bounds.min : bounds.max, stage);
    }
  };

  return { resizing, resizeWithKeyboard, startResize };
}
