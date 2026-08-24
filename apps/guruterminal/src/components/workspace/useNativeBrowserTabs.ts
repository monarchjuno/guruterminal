import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type FormEvent,
} from "react";
import type {
  BrowserBounds,
  BrowserTabEvent,
  GuruTerminalBridge,
} from "../../types";
import type {
  BrowserWorkspaceTab,
  ChatWorkspaceSession,
  ChatWorkspaceTab,
} from "../../chat/workspace";
import { errorMessage } from "../../errors";
import {
  httpAddressError,
  parseCredentialFreeHttpUrl,
} from "../../lib/credentialFreeUrl";

type Options = {
  bridge: GuruTerminalBridge;
  guruId: string;
  threadId: string;
  open: boolean;
  session: ChatWorkspaceSession;
  activeTab?: BrowserWorkspaceTab;
  menuOpen: boolean;
  placement: "side" | "bottom";
  width: number;
  height: number;
  onUpdateTab: (
    tabId: string,
    update: (tab: ChatWorkspaceTab) => ChatWorkspaceTab,
  ) => void;
  onOpenLink: (url: string) => void;
};

const browserBounds = (element: HTMLElement | null): BrowserBounds => {
  const rect = element?.getBoundingClientRect();
  return {
    x: Math.max(0, Math.round(rect?.left ?? 0)),
    y: Math.max(0, Math.round(rect?.top ?? 0)),
    width: Math.max(1, Math.round(rect?.width ?? 1)),
    height: Math.max(1, Math.round(rect?.height ?? 1)),
  };
};

const normalizeBrowserAddress = (raw: string) => {
  const trimmed = raw.trim();
  if (!trimmed || /\s/.test(trimmed)) {
    throw new Error("Enter a web address, not a search query.");
  }
  const candidate = /^https?:\/\//i.test(trimmed)
    ? trimmed
    : `https://${trimmed}`;
  const parsed = parseCredentialFreeHttpUrl(candidate);
  if (!parsed) {
    throw new Error(httpAddressError);
  }
  return parsed.href;
};

export function useNativeBrowserTabs({
  bridge,
  guruId,
  threadId,
  open,
  session,
  activeTab,
  menuOpen,
  placement,
  width,
  height,
  onUpdateTab,
  onOpenLink,
}: Options) {
  const [address, setAddress] = useState("");
  const viewportRef = useRef<HTMLDivElement>(null);
  const lastBoundsRef = useRef<BrowserBounds>({ x: 0, y: 0, width: 1, height: 1 });
  const openingTabsRef = useRef(new Set<string>());
  const managedBrowserIdsRef = useRef(new Set<string>());
  const sentBoundsRef = useRef(new Map<string, string>());
  const boundsFrameRef = useRef<number | null>(null);
  const boundsSyncingRef = useRef(false);
  const boundsDirtyRef = useRef(false);
  const tabsRef = useRef(session.tabs);
  tabsRef.current = session.tabs;
  const surfaceRef = useRef({
    open,
    activeTabId: activeTab?.id,
    menuOpen,
  });
  surfaceRef.current = {
    open,
    activeTabId: activeTab?.id,
    menuOpen,
  };

  useEffect(() => {
    setAddress(activeTab?.url ?? "");
  }, [activeTab?.id, activeTab?.url]);

  const updateFromEvent = useCallback(
    (localTabId: string, event: BrowserTabEvent) => {
      if (event.type === "open_requested") {
        onOpenLink(event.url);
        return;
      }
      onUpdateTab(localTabId, (tab) => {
        if (tab.kind !== "browser") return tab;
        if (event.type === "load_started") {
          return { ...tab, url: event.url, loading: true, error: undefined };
        }
        if (event.type === "load_finished") {
          return { ...tab, url: event.url, loading: false };
        }
        if (event.type === "title_changed") {
          return { ...tab, title: event.title || tab.title };
        }
        if (event.type === "navigation_blocked") {
          return { ...tab, loading: false, error: event.message };
        }
        return {
          ...tab,
          loading: false,
          error: "Downloads are not available in the in-app browser.",
        };
      });
    },
    [onOpenLink, onUpdateTab],
  );

  useEffect(() => {
    if (!open || !activeTab?.url || activeTab.native_id) return;
    if (openingTabsRef.current.has(activeTab.id)) return;
    openingTabsRef.current.add(activeTab.id);
    const localTabId = activeTab.id;
    const bounds = browserBounds(viewportRef.current);
    lastBoundsRef.current = bounds;
    void bridge
      .browserTabOpen(
        { url: activeTab.url, bounds, visible: !menuOpen },
        (event) => updateFromEvent(localTabId, event),
      )
      .then((native) => {
        onUpdateTab(localTabId, (tab) =>
          tab.kind === "browser"
            ? {
                ...tab,
                native_id: native.tab_id,
                url: native.url,
                title: native.title,
                loading: native.loading,
                error: undefined,
              }
            : tab,
        );
      })
      .catch((cause) => {
        onUpdateTab(localTabId, (tab) =>
          tab.kind === "browser"
            ? {
                ...tab,
                loading: false,
                error: errorMessage(cause, "Could not open this web page."),
              }
            : tab,
        );
      })
      .finally(() => openingTabsRef.current.delete(localTabId));
  }, [
    activeTab,
    bridge,
    menuOpen,
    onUpdateTab,
    open,
    updateFromEvent,
  ]);

  const flushBounds = useCallback(async () => {
    if (boundsSyncingRef.current) return;
    boundsSyncingRef.current = true;
    try {
      while (boundsDirtyRef.current) {
        boundsDirtyRef.current = false;
        const bounds = viewportRef.current
          ? browserBounds(viewportRef.current)
          : lastBoundsRef.current;
        lastBoundsRef.current = bounds;

        const currentTabs = tabsRef.current.flatMap((tab) =>
          tab.kind === "browser" && tab.native_id ? [tab] : [],
        );
        const currentIds = new Set(currentTabs.map((tab) => tab.native_id!));
        const removedIds = [...managedBrowserIdsRef.current].filter(
          (tabId) => !currentIds.has(tabId),
        );
        managedBrowserIdsRef.current = currentIds;

        const surface = surfaceRef.current;
        const requests = [
          ...currentTabs.map((tab) => ({
            tab_id: tab.native_id!,
            bounds,
            visible:
              surface.open &&
              tab.id === surface.activeTabId &&
              !surface.menuOpen,
          })),
          ...removedIds.map((tab_id) => ({
            tab_id,
            bounds,
            visible: false,
          })),
        ];

        await Promise.all(
          requests.map(async (request) => {
            const key = [
              request.bounds.x,
              request.bounds.y,
              request.bounds.width,
              request.bounds.height,
              request.visible ? 1 : 0,
            ].join(":");
            if (sentBoundsRef.current.get(request.tab_id) === key) return;
            sentBoundsRef.current.set(request.tab_id, key);
            try {
              await bridge.browserTabSetBounds(request);
            } catch {
              if (sentBoundsRef.current.get(request.tab_id) === key) {
                sentBoundsRef.current.delete(request.tab_id);
              }
            }
          }),
        );

        for (const tabId of removedIds) sentBoundsRef.current.delete(tabId);
      }
    } finally {
      boundsSyncingRef.current = false;
    }
  }, [bridge]);

  const scheduleBounds = useCallback(() => {
    boundsDirtyRef.current = true;
    if (boundsFrameRef.current !== null) return;
    boundsFrameRef.current = window.requestAnimationFrame(() => {
      boundsFrameRef.current = null;
      void flushBounds();
    });
  }, [flushBounds]);

  useLayoutEffect(() => {
    scheduleBounds();
  }, [
    activeTab?.id,
    guruId,
    height,
    menuOpen,
    open,
    placement,
    scheduleBounds,
    session.tabs,
    threadId,
    width,
  ]);

  useEffect(() => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    const observer = new ResizeObserver(scheduleBounds);
    observer.observe(viewport);
    window.addEventListener("resize", scheduleBounds);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", scheduleBounds);
    };
  }, [activeTab?.id, scheduleBounds]);

  useEffect(
    () => () => {
      if (boundsFrameRef.current !== null) {
        window.cancelAnimationFrame(boundsFrameRef.current);
        boundsFrameRef.current = null;
      }
      surfaceRef.current = { ...surfaceRef.current, open: false };
      boundsDirtyRef.current = true;
      void flushBounds();
    },
    [flushBounds],
  );

  const navigate = async (event: FormEvent) => {
    event.preventDefault();
    if (!activeTab) return;
    try {
      const url = normalizeBrowserAddress(address);
      onUpdateTab(activeTab.id, (tab) =>
        tab.kind === "browser"
          ? {
              ...tab,
              url,
              title: new URL(url).hostname,
              loading: true,
              error: undefined,
            }
          : tab,
      );
      if (activeTab.native_id) {
        await bridge.browserTabNavigate(activeTab.native_id, url);
      }
    } catch (cause) {
      onUpdateTab(activeTab.id, (tab) =>
        tab.kind === "browser"
          ? {
              ...tab,
              loading: false,
              error: errorMessage(cause, "Invalid web address."),
            }
          : tab,
      );
    }
  };

  const performAction = async (action: "back" | "forward" | "reload") => {
    if (!activeTab?.native_id) return;
    try {
      if (action === "reload") await bridge.browserTabReload(activeTab.native_id);
      else await bridge.browserTabHistory(activeTab.native_id, action);
    } catch (cause) {
      onUpdateTab(activeTab.id, (tab) =>
        tab.kind === "browser"
          ? { ...tab, error: errorMessage(cause, "Browser navigation failed.") }
          : tab,
      );
    }
  };

  return {
    address,
    setAddress,
    viewportRef,
    navigate,
    performAction,
  };
}
