import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import { PlusIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import type {
  ChatArtifact,
  ChatArtifactRef,
  ChatArtifactView,
  GuruTerminalBridge,
  LibraryRecord,
} from "../../types";
import type {
  ChatWorkspaceSession,
  ChatWorkspaceTab,
  WorkspacePlacement,
} from "../../chat/workspace";
import { workspaceTabButtonId, workspaceTabPanelId } from "../../chat/workspace";
import { errorMessage } from "../../errors";
import { ArtifactContent, chatArtifactRef } from "../artifacts/ArtifactContent";
import { BrowserWorkspaceView } from "./BrowserWorkspaceView";
import { useNativeBrowserTabs } from "./useNativeBrowserTabs";
import {
  useWorkspacePanelResize,
  workspaceOverlayMedia,
  workspacePanelLimits,
} from "./useWorkspacePanelResize";
import { WorkspaceLayoutControls } from "./WorkspaceLayoutControls";
import { WorkspaceMemoryPreview } from "./WorkspaceMemoryPreview";
import { WorkspaceTabBar } from "./WorkspaceTabBar";

type Props = {
  bridge: GuruTerminalBridge;
  guruId: string;
  threadId: string;
  canLoadArtifacts: boolean;
  open: boolean;
  session: ChatWorkspaceSession;
  theme: "light" | "dark";
  width: number;
  height: number;
  placement: WorkspacePlacement;
  maximized: boolean;
  onWidthChange: (width: number) => void;
  onHeightChange: (height: number) => void;
  onPlacementChange: (placement: WorkspacePlacement) => void;
  onMaximizedChange: (maximized: boolean) => void;
  onSelectTab: (tabId: string) => void;
  onUpdateTab: (
    tabId: string,
    update: (tab: ChatWorkspaceTab) => ChatWorkspaceTab,
  ) => void;
  onCloseTab: (tab: ChatWorkspaceTab) => void;
  onOpenArtifact: (artifact: ChatArtifactRef) => void;
  onNewBrowser: () => void;
  onOpenLink: (url: string) => void;
  onClose: () => void;
};

export function ChatWorkspacePanel({
  bridge,
  guruId,
  threadId,
  canLoadArtifacts,
  open,
  session,
  theme,
  width,
  height,
  placement,
  maximized,
  onWidthChange,
  onHeightChange,
  onPlacementChange,
  onMaximizedChange,
  onSelectTab,
  onUpdateTab,
  onCloseTab,
  onOpenArtifact,
  onNewBrowser,
  onOpenLink,
  onClose,
}: Props) {
  const [artifacts, setArtifacts] = useState<ChatArtifact[]>([]);
  const [view, setView] = useState<ChatArtifactView | null>(null);
  const [loadingArtifact, setLoadingArtifact] = useState(false);
  const [memoryRecord, setMemoryRecord] = useState<LibraryRecord | null>(null);
  const [loadingMemory, setLoadingMemory] = useState(false);
  const [memoryError, setMemoryError] = useState<string | null>(null);
  const [catalogError, setCatalogError] = useState<string | null>(null);
  const [readError, setReadError] = useState<string | null>(null);
  const [overlay, setOverlay] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [openingArtifactId, setOpeningArtifactId] = useState<string | null>(
    null,
  );
  const readRequest = useRef(0);
  const memoryReadRequest = useRef(0);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLElement>(null);

  const activeTab = useMemo(
    () => session.tabs.find((tab) => tab.id === session.active_tab_id),
    [session.active_tab_id, session.tabs],
  );
  const activeArtifactTab =
    activeTab?.kind === "artifact" ? activeTab : undefined;
  const activeBrowserTab =
    activeTab?.kind === "browser" ? activeTab : undefined;
  const activeMemoryTab = activeTab?.kind === "memory" ? activeTab : undefined;
  const activeMemoryRecordId = activeMemoryTab?.record_id;

  const { resizing, resizeWithKeyboard, startResize } = useWorkspacePanelResize(
    {
      placement,
      width,
      height,
      onWidthChange,
      onHeightChange,
    },
  );
  const browser = useNativeBrowserTabs({
    bridge,
    guruId,
    threadId,
    open,
    session,
    activeTab: activeBrowserTab,
    menuOpen,
    placement,
    width,
    height,
    onUpdateTab,
    onOpenLink,
  });

  useEffect(() => {
    const query = window.matchMedia(workspaceOverlayMedia);
    const update = () => setOverlay(query.matches);
    update();
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  }, []);

  useEffect(() => {
    if (!open) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !menuOpen) onClose();
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [menuOpen, onClose, open]);

  useEffect(() => {
    if (!open || !overlay) return;
    const panel = panelRef.current;
    const background = panel
      ?.closest(".app-stage")
      ?.querySelector<HTMLElement>(".app-stage-main");
    if (!panel || !background) return;

    const previousFocus =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    const previousInert = Boolean(background.inert);
    background.inert = true;

    const focusableElements = () =>
      Array.from(
        panel.querySelectorAll<HTMLElement>(
          'button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), summary, [href], [tabindex]:not([tabindex="-1"])',
        ),
      ).filter(
        (element) =>
          !element.matches(".artifact-resize-handle") &&
          window.getComputedStyle(element).display !== "none" &&
          window.getComputedStyle(element).visibility !== "hidden",
      );
    const trapFocus = (event: KeyboardEvent) => {
      if (event.key !== "Tab") return;
      const focusable = focusableElements();
      const first = focusable[0];
      const last = focusable.at(-1);
      if (!first || !last) {
        event.preventDefault();
        closeButtonRef.current?.focus();
        return;
      }
      const current = document.activeElement;
      if (event.shiftKey && (current === first || !panel.contains(current))) {
        event.preventDefault();
        last.focus();
      } else if (
        !event.shiftKey &&
        (current === last || !panel.contains(current))
      ) {
        event.preventDefault();
        first.focus();
      }
    };

    document.addEventListener("keydown", trapFocus);
    closeButtonRef.current?.focus();
    return () => {
      document.removeEventListener("keydown", trapFocus);
      background.inert = previousInert;
      const restoreFocus = () => {
        const activeBackground =
          document.querySelector<HTMLElement>(".app-stage-main");
        const restoreTarget =
          activeBackground?.querySelector<HTMLElement>(
            'textarea[aria-label="Message Guru"]:not([disabled])',
          ) ??
          (previousFocus?.isConnected && !previousFocus.matches(":disabled")
            ? previousFocus
            : activeBackground?.querySelector<HTMLElement>(
                'button:not([disabled]), [href], [tabindex]:not([tabindex="-1"])',
              ));
        restoreTarget?.focus();
      };
      restoreFocus();
      queueMicrotask(() => {
        if (document.activeElement === document.body) restoreFocus();
      });
      window.setTimeout(() => {
        if (document.activeElement === document.body) restoreFocus();
      }, 0);
    };
  }, [open, overlay]);

  useEffect(() => {
    if (!canLoadArtifacts) {
      setArtifacts([]);
      setCatalogError(null);
      return;
    }
    let live = true;
    void bridge
      .chatArtifactList(guruId, threadId)
      .then((items) => {
        if (live) {
          setArtifacts(items);
          setCatalogError(null);
        }
      })
      .catch((cause) => {
        if (live)
          setCatalogError(errorMessage(cause, "Could not load saved work."));
      });
    return () => {
      live = false;
    };
  }, [bridge, canLoadArtifacts, guruId, threadId]);

  const readArtifact = useCallback(
    async (artifactId: string) => {
      const request = ++readRequest.current;
      setLoadingArtifact(true);
      setReadError(null);
      try {
        const next = await bridge.chatArtifactRead(guruId, threadId, artifactId);
        if (request === readRequest.current) setView(next);
      } catch (cause) {
        if (request === readRequest.current) {
          setView(null);
          setReadError(errorMessage(cause, "Could not open this item."));
        }
      } finally {
        if (request === readRequest.current) setLoadingArtifact(false);
      }
    },
    [bridge, guruId, threadId],
  );

  useEffect(() => {
    if (!activeArtifactTab) {
      setView(null);
      setReadError(null);
      return;
    }
    void readArtifact(activeArtifactTab.artifact.artifact_id);
  }, [activeArtifactTab, readArtifact]);

  useEffect(() => {
    const request = ++memoryReadRequest.current;
    if (!activeMemoryRecordId) {
      setMemoryRecord(null);
      setLoadingMemory(false);
      setMemoryError(null);
      return;
    }
    setLoadingMemory(true);
    setMemoryError(null);
    void bridge
      .libraryRead(guruId, activeMemoryRecordId)
      .then((record) => {
        if (request === memoryReadRequest.current) setMemoryRecord(record);
      })
      .catch((cause) => {
        if (request === memoryReadRequest.current) {
          setMemoryRecord(null);
          setMemoryError(errorMessage(cause, "Could not open this memory."));
        }
      })
      .finally(() => {
        if (request === memoryReadRequest.current) setLoadingMemory(false);
      });
  }, [activeMemoryRecordId, bridge, guruId]);

  const openCatalogArtifact = async (artifactId: string) => {
    setOpeningArtifactId(artifactId);
    setCatalogError(null);
    try {
      const opened = await bridge.chatArtifactRead(
        guruId,
        threadId,
        artifactId,
      );
      onOpenArtifact(chatArtifactRef(opened));
      setMenuOpen(false);
    } catch (cause) {
      setCatalogError(errorMessage(cause, "Could not open this item."));
    } finally {
      setOpeningArtifactId(null);
    }
  };

  if (!open) return null;

  const tabPanel = activeTab ? (
    <div
      className="workspace-tabpanel"
      role="tabpanel"
      id={workspaceTabPanelId(activeTab.id)}
      aria-labelledby={workspaceTabButtonId(activeTab.id)}
    >
      {activeBrowserTab ? (
        <BrowserWorkspaceView
          tab={activeBrowserTab}
          address={browser.address}
          viewportRef={browser.viewportRef}
          onAddressChange={browser.setAddress}
          onNavigate={(event) => void browser.navigate(event)}
          onAction={(action) => void browser.performAction(action)}
          onOpenExternal={(url) => void bridge.openExternalUrl(url)}
        />
      ) : activeMemoryTab && loadingMemory ? (
        <div className="artifact-panel-state" role="status">
          <Spinner />
          <span>Opening memory…</span>
        </div>
      ) : activeMemoryTab && memoryError ? (
        <div className="artifact-panel-state" role="alert">
          <strong>Could not open memory</strong>
          <p>{memoryError}</p>
        </div>
      ) : activeMemoryTab && memoryRecord ? (
        <WorkspaceMemoryPreview record={memoryRecord} />
      ) : loadingArtifact ? (
        <div className="artifact-panel-state" role="status">
          <Spinner />
          <span>Opening document…</span>
        </div>
      ) : readError ? (
        <div className="artifact-panel-state" role="alert">
          <strong>Could not open workspace item</strong>
          <p>{readError}</p>
        </div>
      ) : view ? (
        <ArtifactContent view={view} theme={theme} onOpenLink={onOpenLink} />
      ) : null}
    </div>
  ) : (
    <div className="artifact-empty workspace-empty">
      <h2>Open alongside the conversation</h2>
      <p>Open a document, chart, or web page beside this chat.</p>
      <Button type="button" variant="outline" onClick={onNewBrowser}>
        <PlusIcon /> New browser tab
      </Button>
    </div>
  );

  return (
    <>
      <button
        type="button"
        className="artifact-panel-backdrop"
        aria-label="Close chat workspace"
        tabIndex={-1}
        onClick={onClose}
      />
      <aside
        ref={panelRef}
        className="artifact-panel workspace-panel"
        data-placement={placement}
        data-maximized={maximized || undefined}
        data-resizing={resizing || undefined}
        style={
          {
            "--artifact-panel-width": `${width}px`,
            "--artifact-panel-height": `${height}px`,
          } as CSSProperties
        }
        aria-label="Chat workspace panel"
        aria-modal={overlay || undefined}
        role={overlay ? "dialog" : "complementary"}
      >
        {!maximized && (
          <div
            className="artifact-resize-handle"
            role="separator"
            aria-label="Resize chat workspace"
            aria-orientation={placement === "side" ? "vertical" : "horizontal"}
            aria-valuemin={workspacePanelLimits[placement].min}
            aria-valuemax={workspacePanelLimits[placement].max}
            aria-valuenow={placement === "side" ? width : height}
            tabIndex={0}
            onPointerDown={startResize}
            onKeyDown={resizeWithKeyboard}
          />
        )}
        <WorkspaceTabBar
          session={session}
          activeTab={activeTab}
          artifacts={artifacts}
          catalogError={catalogError}
          menuOpen={menuOpen}
          openingArtifactId={openingArtifactId}
          closeButtonRef={closeButtonRef}
          onMenuOpenChange={setMenuOpen}
          onSelectTab={onSelectTab}
          onCloseTab={onCloseTab}
          onNewBrowser={onNewBrowser}
          onOpenArtifact={(artifactId) => void openCatalogArtifact(artifactId)}
          onClose={onClose}
        />
        <header className="artifact-panel-header">
          <WorkspaceLayoutControls
            placement={placement}
            maximized={maximized}
            onPlacementChange={(next) => {
              onPlacementChange(next);
              onMaximizedChange(false);
            }}
            onMaximizedChange={onMaximizedChange}
          />
        </header>
        {tabPanel}
      </aside>
    </>
  );
}
