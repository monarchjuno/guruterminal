import type { ChatArtifactRef } from "../types";

export type WorkspacePlacement = "side" | "bottom";

export type ArtifactWorkspaceTab = {
  id: string;
  kind: "artifact";
  artifact: ChatArtifactRef;
};

export type BrowserWorkspaceTab = {
  id: string;
  kind: "browser";
  native_id?: string;
  url: string;
  title: string;
  loading: boolean;
  error?: string;
};

export type MemoryWorkspaceTab = {
  id: string;
  kind: "memory";
  record_id: string;
  title: string;
};

export type ChatWorkspaceTab =
  | ArtifactWorkspaceTab
  | BrowserWorkspaceTab
  | MemoryWorkspaceTab;

export type ChatWorkspaceSession = {
  tabs: ChatWorkspaceTab[];
  active_tab_id?: string;
};

export const workspaceTabButtonId = (tabId: string) => `workspace-tab-${tabId}`;

export const workspaceTabPanelId = (tabId: string) =>
  `workspace-tabpanel-${tabId}`;

export const workspaceTabTitle = (tab: ChatWorkspaceTab) =>
  tab.kind === "artifact" ? tab.artifact.title : tab.title;

export const emptyWorkspaceSession = (): ChatWorkspaceSession => ({ tabs: [] });

export const activateWorkspaceTab = (
  session: ChatWorkspaceSession,
  tab: ChatWorkspaceTab,
): ChatWorkspaceSession => {
  const existing = session.tabs.findIndex((item) => item.id === tab.id);
  const tabs = [...session.tabs];
  if (existing === -1) tabs.push(tab);
  else tabs[existing] = tab;
  return { tabs, active_tab_id: tab.id };
};

export const artifactWorkspaceTab = (
  artifact: ChatArtifactRef,
): ArtifactWorkspaceTab => ({
  id: `artifact:${artifact.artifact_id}`,
  kind: "artifact",
  artifact,
});

export const browserWorkspaceTab = (url = ""): BrowserWorkspaceTab => {
  let title = "New tab";
  if (url) {
    try {
      title = new URL(url).hostname || "Web page";
    } catch {
      title = "Web page";
    }
  }
  return {
    id: `browser:${crypto.randomUUID()}`,
    kind: "browser",
    url,
    title,
    loading: Boolean(url),
  };
};

export const memoryWorkspaceTab = (
  recordId: string,
  title: string,
): MemoryWorkspaceTab => ({
  id: `memory:${recordId}`,
  kind: "memory",
  record_id: recordId,
  title,
});
