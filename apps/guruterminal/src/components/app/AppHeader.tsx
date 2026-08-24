import { PanelRightIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { SidebarTrigger } from "@/components/ui/sidebar";
import type { GuruSummary } from "../../types";
import { appTabDefinition, type AppTab } from "../../navigation";

type Props = {
  tab: AppTab;
  title: string;
  guru: GuruSummary | null;
  workspaceOpen: boolean;
  onToggleWorkspace: () => void;
};

export function AppHeader({
  tab,
  title,
  guru,
  workspaceOpen,
  onToggleWorkspace,
}: Props) {
  const currentTab = appTabDefinition(tab);

  return (
    <header className="app-header" data-tauri-drag-region="deep">
      <div className="app-header-context">
        <SidebarTrigger className="app-header-sidebar-trigger" />
        <div className="context-title">
          <strong>{title}</strong>
        </div>
      </div>

      <div className="guru-header-tools">
        {guru && currentTab.guruScoped && (
          <div className="guru-presence" title={guru.philosophy}>
            <i style={{ background: guru.accent }} />
            <span>
              <small>Agent</small>
              <strong>{guru.name}</strong>
            </span>
          </div>
        )}
        {tab === "chat" && !workspaceOpen && (
          <Button
            type="button"
            size="icon"
            variant="ghost"
            className="artifact-panel-toggle"
            aria-label="Show chat workspace"
            title="Show chat workspace"
            onClick={onToggleWorkspace}
          >
            <PanelRightIcon />
          </Button>
        )}
      </div>
    </header>
  );
}
