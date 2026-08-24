import {
  ArrowLeftIcon,
  ChevronRightIcon,
  FolderIcon,
  MessageSquareIcon,
  PencilIcon,
  PlusIcon,
  Trash2Icon,
} from "lucide-react";
import { useState } from "react";
import { Spinner } from "@/components/ui/spinner";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupAction,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuAction,
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem,
  SidebarRail,
  SidebarSeparator,
  useSidebar,
} from "@/components/ui/sidebar";
import {
  SETTINGS_SECTIONS,
  appTabsInGroup,
  type AppTab,
  type SettingsSection,
} from "../../navigation";
import { chatSessionKey } from "../../chat/sessionRegistry";
import type {
  ChatThread,
  GuruSummary,
} from "../../types";

const mainTabs = [
  ...appTabsInGroup("workspace"),
  ...appTabsInGroup("ecosystem"),
];
const settingsTab = appTabsInGroup("footer")[0];

type Props = {
  tab: AppTab;
  gurus: GuruSummary[];
  selectedGuru: GuruSummary | null;
  loading: boolean;
  threads: ChatThread[];
  activeThreadId: string | null;
  runningGuruIds: ReadonlySet<string>;
  runningThreadKeys: ReadonlySet<string>;
  settingsSection: SettingsSection;
  updateAvailable: boolean;
  onTabChange: (tab: AppTab) => void;
  onSettingsSectionChange: (section: SettingsSection) => void;
  onExitSettings: () => void;
  onSelectGuru: (guruId: string) => void;
  onCreateGuru: () => void;
  onCreateThread: (guruId: string) => void;
  onSelectThread: (threadId: string) => void;
  onRenameThread: (thread: ChatThread) => void;
  onDeleteThread: (thread: ChatThread) => void;
};

export function AppNavigation({
  tab,
  gurus,
  selectedGuru,
  loading,
  threads,
  activeThreadId,
  runningGuruIds,
  runningThreadKeys,
  settingsSection,
  updateAvailable,
  onTabChange,
  onSettingsSectionChange,
  onExitSettings,
  onSelectGuru,
  onCreateGuru,
  onCreateThread,
  onSelectThread,
  onRenameThread,
  onDeleteThread,
}: Props) {
  const { isMobile, setOpenMobile } = useSidebar();
  const [collapsedSessionGuruIds, setCollapsedSessionGuruIds] = useState<
    ReadonlySet<string>
  >(() => new Set());
  const closeMobileNavigation = () => {
    if (isMobile) setOpenMobile(false);
  };
  const navigateTo = (nextTab: AppTab) => {
    onTabChange(nextTab);
    closeMobileNavigation();
  };
  const expandSessions = (guruId: string) => {
    setCollapsedSessionGuruIds((current) => {
      if (!current.has(guruId)) return current;
      const next = new Set(current);
      next.delete(guruId);
      return next;
    });
  };
  const toggleSessions = (guruId: string) => {
    setCollapsedSessionGuruIds((current) => {
      const next = new Set(current);
      if (next.has(guruId)) next.delete(guruId);
      else next.add(guruId);
      return next;
    });
  };

  if (tab === "settings") {
    return (
      <Sidebar
        className="app-navigation"
        collapsible="offcanvas"
        aria-label="Settings navigation"
      >
        <SidebarHeader className="gap-3 p-3" data-tauri-drag-region="deep">
          <div className="app-brand">
            <strong>Settings</strong>
          </div>
        </SidebarHeader>

        <SidebarContent>
          <SidebarGroup className="primary-navigation">
            <SidebarGroupLabel>Settings</SidebarGroupLabel>
            <SidebarGroupContent>
              <nav aria-label="Settings sections">
                <SidebarMenu className="gap-1">
                  {SETTINGS_SECTIONS.map((item) => {
                    const SectionIcon = item.icon;
                    return (
                      <SidebarMenuItem key={item.id}>
                        <SidebarMenuButton
                          id={`settings-section-${item.id}`}
                          type="button"
                          className="workspace-nav-item"
                          aria-label={item.label}
                          isActive={settingsSection === item.id}
                          aria-current={
                            settingsSection === item.id ? "page" : undefined
                          }
                          aria-controls="main-panel-settings"
                          onClick={() => {
                            onSettingsSectionChange(item.id);
                            closeMobileNavigation();
                          }}
                        >
                          <SectionIcon />
                          <span>{item.label}</span>
                          {item.id === "updates" && updateAvailable ? (
                            <SidebarMenuBadge aria-label="Update available">
                              New
                            </SidebarMenuBadge>
                          ) : null}
                        </SidebarMenuButton>
                      </SidebarMenuItem>
                    );
                  })}
                </SidebarMenu>
              </nav>
            </SidebarGroupContent>
          </SidebarGroup>
        </SidebarContent>

        <SidebarFooter className="app-settings-footer border-t border-sidebar-border">
          <SidebarMenu>
            <SidebarMenuItem>
              <SidebarMenuButton
                type="button"
                size="sm"
                className="app-settings-entry workspace-nav-item"
                onClick={() => {
                  onExitSettings();
                  closeMobileNavigation();
                }}
              >
                <ArrowLeftIcon />
                <span>Back to app</span>
              </SidebarMenuButton>
            </SidebarMenuItem>
          </SidebarMenu>
        </SidebarFooter>
        <SidebarRail />
      </Sidebar>
    );
  }

  return (
    <Sidebar
      className="app-navigation"
      collapsible="offcanvas"
      aria-label="Application navigation"
    >
      <SidebarHeader className="gap-3 p-3" data-tauri-drag-region="deep">
        <div className="app-brand">
          <div>
            <strong>Guru Terminal</strong>
          </div>
        </div>

      </SidebarHeader>

      <SidebarContent>
        <SidebarGroup className="primary-navigation">
          <SidebarGroupLabel className="sr-only">Main</SidebarGroupLabel>
          <SidebarGroupContent>
            <nav aria-label="Main views">
              <SidebarMenu className="gap-1">
                {mainTabs.map((item) => {
                  const TabIcon = item.icon;
                  return (
                    <SidebarMenuItem key={item.id}>
                      <SidebarMenuButton
                        id={`main-tab-${item.id}`}
                        type="button"
                        size="default"
                        className="workspace-nav-item"
                        isActive={tab === item.id}
                        aria-current={tab === item.id ? "page" : undefined}
                        aria-controls={`main-panel-${item.id}`}
                        onClick={() => navigateTo(item.id)}
                      >
                        <TabIcon />
                        <span>{item.label}</span>
                      </SidebarMenuButton>
                    </SidebarMenuItem>
                  );
                })}
              </SidebarMenu>
            </nav>
          </SidebarGroupContent>
        </SidebarGroup>

        {tab !== "agents" ? (
          <>
            <SidebarSeparator />
            <SidebarGroup className="guru-navigation min-h-0 flex-1">
              <SidebarGroupLabel>Agents</SidebarGroupLabel>
              <SidebarGroupAction
                type="button"
                className="guru-new-agent-action"
                aria-label="New agent"
                title="New agent"
                disabled={loading}
                onClick={() => {
                  onCreateGuru();
                  closeMobileNavigation();
                }}
              >
                <PlusIcon />
              </SidebarGroupAction>
              <SidebarGroupContent className="min-h-0 overflow-y-auto">
                <nav aria-label="Gurus">
              <SidebarMenu className="gap-1">
                {gurus.length > 0 ? (
                  gurus.map((guru) => {
                    const guruIsRunning = runningGuruIds.has(guru.id);
                    const guruIsSelected = guru.id === selectedGuru?.id;
                    const hasSessions = guruIsSelected && threads.length > 0;
                    const sessionsExpanded =
                      hasSessions && !collapsedSessionGuruIds.has(guru.id);
                    return (
                      <SidebarMenuItem
                        key={guru.id}
                        className={
                          guruIsRunning ? "guru-session-running" : undefined
                        }
                      >
                        <SidebarMenuButton
                          type="button"
                          className="guru-nav-item"
                          isActive={guruIsSelected}
                          aria-current={
                            guruIsSelected ? "page" : undefined
                          }
                          aria-expanded={hasSessions ? sessionsExpanded : undefined}
                          aria-controls={
                            sessionsExpanded
                              ? `guru-sessions-${guru.id}`
                              : undefined
                          }
                          disabled={loading}
                          title={guru.name}
                          onClick={() => {
                            if (guruIsSelected) {
                              if (hasSessions) toggleSessions(guru.id);
                              return;
                            }
                            expandSessions(guru.id);
                            onSelectGuru(guru.id);
                            closeMobileNavigation();
                          }}
                        >
                          <ChevronRightIcon
                            className="guru-session-chevron"
                            aria-hidden="true"
                          />
                          <FolderIcon style={{ color: guru.accent }} />
                          <span>{guru.name}</span>
                        </SidebarMenuButton>
                        <SidebarMenuAction
                          type="button"
                          className="guru-new-thread-action"
                          aria-label={`New session for ${guru.name}`}
                          title={`New session for ${guru.name}`}
                          disabled={loading}
                          onClick={() => {
                            expandSessions(guru.id);
                            onCreateThread(guru.id);
                            closeMobileNavigation();
                          }}
                        >
                          <PlusIcon />
                        </SidebarMenuAction>
                        {guruIsRunning ? (
                          <span
                            className="guru-running-indicator"
                            role="status"
                            aria-label={`${guru.name} has active sessions`}
                            title="Active session"
                          />
                        ) : null}
                        {sessionsExpanded && threads.length > 0 ? (
                          <SidebarMenuSub
                            id={`guru-sessions-${guru.id}`}
                            className="guru-thread-list"
                          >
                            {threads.map((thread) => {
                              const threadIsRunning = runningThreadKeys.has(
                                chatSessionKey(guru.id, thread.id),
                              );
                              return (
                                <SidebarMenuSubItem
                                  key={thread.id}
                                  className={`guru-thread-row${
                                    threadIsRunning
                                      ? " guru-thread-running"
                                      : ""
                                  }`}
                                >
                                  <SidebarMenuSubButton
                                    asChild
                                    isActive={thread.id === activeThreadId}
                                    className="guru-thread-item"
                                  >
                                    <button
                                      type="button"
                                      onClick={() => {
                                        onSelectThread(thread.id);
                                        closeMobileNavigation();
                                      }}
                                    >
                                      {threadIsRunning ? (
                                        <Spinner aria-hidden="true" />
                                      ) : (
                                        <MessageSquareIcon />
                                      )}
                                      <span id={`thread-title-${thread.id}`}>
                                        {thread.title}
                                      </span>
                                    </button>
                                  </SidebarMenuSubButton>
                                  <div className="guru-thread-actions">
                                    <button
                                      type="button"
                                      aria-label="Rename session"
                                      aria-describedby={`thread-title-${thread.id}`}
                                      title="Rename session"
                                      onClick={() => onRenameThread(thread)}
                                    >
                                      <PencilIcon />
                                    </button>
                                    <button
                                      type="button"
                                      className="guru-thread-delete"
                                      aria-label="Delete session"
                                      aria-describedby={`thread-title-${thread.id}`}
                                      title="Delete session"
                                      onClick={() => onDeleteThread(thread)}
                                    >
                                      <Trash2Icon />
                                    </button>
                                  </div>
                                  {threadIsRunning ? (
                                    <span
                                      className="thread-running-indicator"
                                      role="status"
                                      aria-label={`${thread.title} is running`}
                                      title="Running"
                                    />
                                  ) : null}
                                </SidebarMenuSubItem>
                              );
                            })}
                          </SidebarMenuSub>
                        ) : null}
                      </SidebarMenuItem>
                    );
                  })
                ) : (
                  <li className="guru-list-empty">No agents yet</li>
                )}
              </SidebarMenu>
                </nav>
              </SidebarGroupContent>
            </SidebarGroup>
          </>
        ) : null}
      </SidebarContent>

      <SidebarFooter className="app-settings-footer border-t border-sidebar-border">
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton
              id="main-tab-settings"
              type="button"
              size="sm"
              className="app-settings-entry workspace-nav-item"
              aria-label="Settings"
              aria-controls="main-panel-settings"
              onClick={() => navigateTo("settings")}
            >
              <settingsTab.icon />
              <span>Settings</span>
              {updateAvailable ? (
                <SidebarMenuBadge aria-label="Update available">
                  New
                </SidebarMenuBadge>
              ) : null}
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarFooter>
      <SidebarRail />
    </Sidebar>
  );
}
