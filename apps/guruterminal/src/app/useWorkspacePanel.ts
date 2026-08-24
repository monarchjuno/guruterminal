import { useCallback, useRef, useState } from "react";
import type { ChatArtifactRef, GuruTerminalBridge } from "../types";
import type { AppTab } from "../navigation";
import { chatSessionKey } from "../chat/sessionRegistry";
import {
  activateWorkspaceTab,
  artifactWorkspaceTab,
  browserWorkspaceTab,
  emptyWorkspaceSession,
  memoryWorkspaceTab,
  type BrowserWorkspaceTab,
  type ChatWorkspaceSession,
  type ChatWorkspaceTab,
  type WorkspacePlacement,
} from "../chat/workspace";

export type VisibleWorkspace = {
  guruId: string | null;
  threadId: string | null;
  tab: AppTab;
};

const isNativeBrowserTab = (
  tab: ChatWorkspaceTab,
): tab is BrowserWorkspaceTab =>
  tab.kind === "browser" && Boolean(tab.native_id);

/** Chat workspace panel: per-thread tab sessions, open state, and geometry. */
export function useWorkspacePanel(bridge: GuruTerminalBridge) {
  const [open, setOpen] = useState(false);
  const [panelWidth, setPanelWidth] = useState(460);
  const [panelHeight, setPanelHeight] = useState(420);
  const [placement, setPlacement] = useState<WorkspacePlacement>("side");
  const [maximized, setMaximized] = useState(false);
  const [sessions, setSessions] = useState<
    Record<string, ChatWorkspaceSession>
  >({});
  const sessionsRef = useRef(sessions);
  sessionsRef.current = sessions;
  const visibleRef = useRef<VisibleWorkspace>({
    guruId: null,
    threadId: null,
    tab: "chat",
  });

  const activateTab = useCallback(
    (guruId: string, threadId: string, tab: ChatWorkspaceTab) => {
      const key = chatSessionKey(guruId, threadId);
      setSessions((current) => ({
        ...current,
        [key]: activateWorkspaceTab(
          current[key] ?? emptyWorkspaceSession(),
          tab,
        ),
      }));
    },
    [],
  );

  /** Background publish from a run: opens the panel only when the session is visible. */
  const publishArtifact = useCallback(
    (guruId: string, threadId: string, artifact: ChatArtifactRef) => {
      activateTab(guruId, threadId, artifactWorkspaceTab(artifact));
      const visible = visibleRef.current;
      if (
        visible.tab === "chat" &&
        visible.guruId === guruId &&
        visible.threadId === threadId
      ) {
        setOpen(true);
      }
    },
    [activateTab],
  );

  const openArtifact = useCallback(
    (guruId: string, threadId: string, artifact: ChatArtifactRef) => {
      activateTab(guruId, threadId, artifactWorkspaceTab(artifact));
      setOpen(true);
    },
    [activateTab],
  );

  const openMemory = useCallback(
    (guruId: string, threadId: string, recordId: string, title: string) => {
      activateTab(guruId, threadId, memoryWorkspaceTab(recordId, title));
      setOpen(true);
    },
    [activateTab],
  );

  const openBrowser = useCallback(
    (guruId: string, threadId: string, url = "") => {
      activateTab(guruId, threadId, browserWorkspaceTab(url));
      setOpen(true);
    },
    [activateTab],
  );

  const updateTab = useCallback(
    (
      key: string,
      tabId: string,
      update: (tab: ChatWorkspaceTab) => ChatWorkspaceTab,
    ) => {
      setSessions((current) => {
        const session = current[key] ?? emptyWorkspaceSession();
        return {
          ...current,
          [key]: {
            ...session,
            tabs: session.tabs.map((tab) =>
              tab.id === tabId ? update(tab) : tab,
            ),
          },
        };
      });
    },
    [],
  );

  const selectTab = useCallback((key: string, tabId: string) => {
    setSessions((current) => ({
      ...current,
      [key]: {
        ...(current[key] ?? emptyWorkspaceSession()),
        active_tab_id: tabId,
      },
    }));
  }, []);

  const closeTab = useCallback(
    (key: string, tab: ChatWorkspaceTab) => {
      if (tab.kind === "browser" && tab.native_id) {
        void bridge.browserTabClose(tab.native_id).catch(() => undefined);
      }
      let becameEmpty = false;
      setSessions((current) => {
        const session = current[key] ?? emptyWorkspaceSession();
        const index = session.tabs.findIndex((item) => item.id === tab.id);
        const tabs = session.tabs.filter((item) => item.id !== tab.id);
        const active_tab_id =
          session.active_tab_id === tab.id
            ? tabs[Math.min(Math.max(index, 0), tabs.length - 1)]?.id
            : session.active_tab_id;
        becameEmpty = tabs.length === 0;
        return { ...current, [key]: { tabs, active_tab_id } };
      });
      if (becameEmpty) setOpen(false);
    },
    [bridge],
  );

  /** Moves a pending session (e.g. the empty-thread draft) onto a persisted thread. */
  const adoptSession = useCallback((fromKey: string, toKey: string) => {
    setSessions((current) => {
      const pending = current[fromKey];
      if (!pending) return current;
      const next = { ...current, [toKey]: pending };
      delete next[fromKey];
      return next;
    });
  }, []);

  /** Closes native browser tabs and drops the session for one thread. */
  const removeSession = useCallback(
    async (guruId: string, threadId: string) => {
      const key = chatSessionKey(guruId, threadId);
      const browserTabs = (sessionsRef.current[key]?.tabs ?? []).filter(
        isNativeBrowserTab,
      );
      await Promise.all(
        browserTabs.map((tab) =>
          bridge.browserTabClose(tab.native_id!).catch(() => undefined),
        ),
      );
      setSessions((current) => {
        const next = { ...current };
        delete next[key];
        return next;
      });
      if (
        visibleRef.current.guruId === guruId &&
        visibleRef.current.threadId === threadId
      ) {
        setOpen(false);
      }
    },
    [bridge],
  );

  /** Closes native browser tabs and drops every session owned by one guru. */
  const removeGuruSessions = useCallback(
    async (guruId: string) => {
      const prefix = `${encodeURIComponent(guruId)}:`;
      const browserTabs = Object.entries(sessionsRef.current)
        .filter(([key]) => key.startsWith(prefix))
        .flatMap(([, session]) => session.tabs)
        .filter(isNativeBrowserTab);
      await Promise.all(
        browserTabs.map((tab) =>
          bridge.browserTabClose(tab.native_id!).catch(() => undefined),
        ),
      );
      setSessions((current) =>
        Object.fromEntries(
          Object.entries(current).filter(([key]) => !key.startsWith(prefix)),
        ),
      );
    },
    [bridge],
  );

  const close = useCallback(() => setOpen(false), []);
  const toggle = useCallback(() => setOpen((current) => !current), []);

  return {
    open,
    setOpen,
    close,
    toggle,
    panelWidth,
    setPanelWidth,
    panelHeight,
    setPanelHeight,
    placement,
    setPlacement,
    maximized,
    setMaximized,
    sessions,
    visibleRef,
    publishArtifact,
    openArtifact,
    openMemory,
    openBrowser,
    updateTab,
    selectTab,
    closeTab,
    adoptSession,
    removeSession,
    removeGuruSessions,
  };
}

export type WorkspacePanel = ReturnType<typeof useWorkspacePanel>;
