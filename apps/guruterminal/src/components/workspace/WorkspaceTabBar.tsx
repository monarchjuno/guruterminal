import type { KeyboardEvent, Ref } from "react";
import {
  BookOpenIcon,
  ChartNoAxesCombinedIcon,
  FileTextIcon,
  GlobeIcon,
  PlusIcon,
  XIcon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import type { ChatArtifact } from "../../types";
import type {
  ChatWorkspaceSession,
  ChatWorkspaceTab,
} from "../../chat/workspace";
import {
  workspaceTabButtonId,
  workspaceTabPanelId,
  workspaceTabTitle,
} from "../../chat/workspace";

const RECENT_ARTIFACT_LIMIT = 6;

type Props = {
  session: ChatWorkspaceSession;
  activeTab?: ChatWorkspaceTab;
  artifacts: ChatArtifact[];
  catalogError: string | null;
  menuOpen: boolean;
  openingArtifactId: string | null;
  closeButtonRef: Ref<HTMLButtonElement>;
  onMenuOpenChange: (open: boolean) => void;
  onSelectTab: (tabId: string) => void;
  onCloseTab: (tab: ChatWorkspaceTab) => void;
  onNewBrowser: () => void;
  onOpenArtifact: (artifactId: string) => void;
  onClose: () => void;
};

const tabIcon = (tab: ChatWorkspaceTab) => {
  if (tab.kind === "browser") return <GlobeIcon />;
  if (tab.kind === "memory") return <BookOpenIcon />;
  if (tab.artifact.kind === "chart") return <ChartNoAxesCombinedIcon />;
  return <FileTextIcon />;
};

export function WorkspaceTabBar({
  session,
  activeTab,
  artifacts,
  catalogError,
  menuOpen,
  openingArtifactId,
  closeButtonRef,
  onMenuOpenChange,
  onSelectTab,
  onCloseTab,
  onNewBrowser,
  onOpenArtifact,
  onClose,
}: Props) {
  const recentArtifacts = [...artifacts]
    .sort((left, right) => right.updated_at_ms - left.updated_at_ms)
    .slice(0, RECENT_ARTIFACT_LIMIT);

  const moveTabFocus = (event: KeyboardEvent, tabId: string) => {
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    const current = session.tabs.findIndex((tab) => tab.id === tabId);
    const next =
      event.key === "Home"
        ? 0
        : event.key === "End"
          ? session.tabs.length - 1
          : (current +
              (event.key === "ArrowRight" ? 1 : -1) +
              session.tabs.length) %
            session.tabs.length;
    const nextTab = session.tabs[next];
    if (nextTab) {
      onSelectTab(nextTab.id);
      document.getElementById(workspaceTabButtonId(nextTab.id))?.focus();
    }
  };

  return (
    <div className="workspace-tabbar">
      <div
        className="workspace-tabs"
        role="tablist"
        aria-label="Open workspace tabs"
      >
        {session.tabs.map((tab) => {
          const title = workspaceTabTitle(tab);
          const selected = tab.id === activeTab?.id;
          return (
            <div
              className="workspace-tab"
              data-active={selected || undefined}
              key={tab.id}
            >
              <button
                id={workspaceTabButtonId(tab.id)}
                type="button"
                role="tab"
                aria-selected={selected}
                aria-controls={workspaceTabPanelId(tab.id)}
                tabIndex={selected ? 0 : -1}
                onClick={() => onSelectTab(tab.id)}
                onKeyDown={(event) => moveTabFocus(event, tab.id)}
              >
                {tabIcon(tab)}
                <span>{title}</span>
                {tab.kind === "browser" && tab.loading && <Spinner />}
              </button>
              <button
                type="button"
                className="workspace-tab-close"
                aria-label={`Close ${title} tab`}
                onClick={() => onCloseTab(tab)}
              >
                <XIcon />
              </button>
            </div>
          );
        })}
      </div>
      <details
        className="workspace-add"
        open={menuOpen}
        onToggle={(event) => onMenuOpenChange(event.currentTarget.open)}
      >
        <summary aria-label="Open workspace item" title="Open workspace item">
          <PlusIcon />
        </summary>
        <div className="workspace-add-menu" role="menu">
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              onNewBrowser();
              onMenuOpenChange(false);
            }}
          >
            <GlobeIcon />
            <span>
              <strong>New browser tab</strong>
              <small>Open an HTTP or HTTPS address</small>
            </span>
          </button>
          {catalogError ? (
            <p className="workspace-add-error" role="alert">
              {catalogError}
            </p>
          ) : null}
          {recentArtifacts.map((artifact) => (
            <button
              type="button"
              role="menuitem"
              key={artifact.id}
              disabled={openingArtifactId === artifact.id}
              onClick={() => onOpenArtifact(artifact.id)}
            >
              {artifact.kind === "chart" ? (
                <ChartNoAxesCombinedIcon />
              ) : (
                <FileTextIcon />
              )}
              <span>
                <strong>{artifact.title}</strong>
                <small>{artifact.kind === "chart" ? "Chart" : "Document"}</small>
              </span>
            </button>
          ))}
        </div>
      </details>
      <Button
        ref={closeButtonRef}
        className="workspace-panel-close"
        type="button"
        size="icon"
        variant="ghost"
        aria-label="Close chat workspace"
        onClick={onClose}
      >
        <XIcon />
      </Button>
    </div>
  );
}
