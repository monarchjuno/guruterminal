import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
  type CSSProperties,
} from "react";
import { createGuruTerminalBridge } from "./bridge";
import { ChatView, type ChatWorkspaceActions } from "./components/ChatView";
import { AppHeader } from "./components/app/AppHeader";
import { GuruAvailabilityBoundary } from "./components/app/GuruAvailabilityBoundary";
import { AppNavigation } from "./components/app/AppNavigation";
import { MacTitlebarControls } from "./components/app/MacTitlebarControls";
import { AgentsView } from "./components/agents/AgentsView";
import { ChatWorkspacePanel } from "./components/workspace/ChatWorkspacePanel";
import { Icon } from "./components/Icon";
import { LibraryView } from "./components/LibraryView";
import { MarketplaceView } from "./components/MarketplaceView";
import { SettingsView } from "./components/SettingsView";
import { useAppUpdate } from "./components/settings/useAppUpdate";
import { SidebarInset, SidebarProvider } from "@/components/ui/sidebar";
import { Spinner } from "@/components/ui/spinner";
import { TooltipProvider } from "@/components/ui/tooltip";
import type {
  ChatThread,
  GuruTerminalBridge,
  ModelRunSelection,
} from "./types";
import { resolveModelRunSelection } from "./modelSelection";
import { useTheme } from "./theme";
import type { AppTab, SettingsSection } from "./navigation";
import { chatSessionKey } from "./chat/sessionRegistry";
import { emptyWorkspaceSession } from "./chat/workspace";
import { EMPTY_CHAT_THREAD_ID } from "./app/emptyChat";
import { AppDialogs, useAppDialogs } from "./app/AppDialogs";
import { useChatSessions } from "./app/useChatSessions";
import { useGuruDirectory } from "./app/useGuruDirectory";
import { useModelCatalog } from "./app/useModelCatalog";
import { useThreadActions } from "./app/useThreadActions";
import { useWorkspacePanel } from "./app/useWorkspacePanel";

type MemoryLocation = { record_id: string };

export function App({ bridge: providedBridge }: { bridge?: GuruTerminalBridge }) {
  const [bridge] = useState(() => providedBridge ?? createGuruTerminalBridge());
  const appUpdate = useAppUpdate(bridge);
  const [tab, setTab] = useState<AppTab>("chat");
  const [settingsSection, setSettingsSection] =
    useState<SettingsSection>("model");
  const [visitedWorkspaceTabs, setVisitedWorkspaceTabs] = useState<
    ReadonlySet<string>
  >(() => new Set());
  const [requestedMemory, setRequestedMemory] = useState<MemoryLocation | null>(
    null,
  );
  const {
    preference: theme,
    resolved: resolvedTheme,
    setPreference: setTheme,
  } = useTheme();
  const settingsReturnTabRef = useRef<AppTab>("chat");

  const dialogs = useAppDialogs();
  const model = useModelCatalog(bridge);
  const workspace = useWorkspacePanel(bridge);
  const chat = useChatSessions(bridge);
  const clearPendingIntents = useCallback(() => {
    setRequestedMemory(null);
  }, []);
  const guru = useGuruDirectory(bridge, {
    chat,
    removeGuruSessions: workspace.removeGuruSessions,
    closeWorkspace: workspace.close,
    onSelectionStarted: dialogs.closeThreadDialogs,
    onSelectionReset: clearPendingIntents,
  });
  const threadActions = useThreadActions(bridge, { chat, guru, workspace });
  chat.handlersRef.current = {
    publishArtifact: workspace.publishArtifact,
    refreshGuruAvailability: guru.refreshGuruAvailability,
    applyRecoveredGuru: guru.applyRecoveredGuru,
  };
  const { gurus, selectedGuru } = guru;

  useEffect(() => {
    void bridge.browserTabsReset().catch(() => undefined);
  }, [bridge]);

  useEffect(() => {
    if (!selectedGuru || tab !== "library") return;
    const key = `${selectedGuru.id}:${tab}`;
    setVisitedWorkspaceTabs((current) => {
      if (current.has(key)) return current;
      const next = new Set(current);
      next.add(key);
      return next;
    });
  }, [selectedGuru, tab]);

  const requestConsumed = useCallback(() => {
    setRequestedMemory(null);
  }, []);

  const createThread = useCallback(
    () =>
      selectedGuru
        ? threadActions.createThreadForGuru(selectedGuru.id)
        : Promise.resolve(null),
    [threadActions.createThreadForGuru, selectedGuru],
  );

  const modelSelection = resolveModelRunSelection(
    model.catalog,
    selectedGuru
      ? (model.selections[selectedGuru.id]?.model_profile_id ??
          selectedGuru.last_model_profile_id)
      : undefined,
    selectedGuru ? model.selections[selectedGuru.id]?.thinking_level : undefined,
    selectedGuru ? model.selections[selectedGuru.id]?.run_options : undefined,
  );

  const accentStyle = {
    "--guru-accent": selectedGuru?.accent ?? "#a4530c",
  } as CSSProperties;
  const selectedGuruId = selectedGuru?.id ?? null;
  const changeSelectedModel = useCallback(
    (selection: ModelRunSelection) => {
      if (selectedGuruId) model.changeSelection(selectedGuruId, selection);
    },
    [model.changeSelection, selectedGuruId],
  );
  const recordSelectedModel = useCallback(
    (selection: ModelRunSelection) => {
      if (!selectedGuruId) return;
      model.changeSelection(selectedGuruId, selection);
      guru.recordLastModel(selectedGuruId, selection.model_profile_id);
    },
    [model.changeSelection, guru.recordLastModel, selectedGuruId],
  );
  const threads = selectedGuruId
    ? (chat.threadsByGuru[selectedGuruId] ?? [])
    : [];
  const activeThreadId = selectedGuruId
    ? (chat.activeThreadIds[selectedGuruId] ?? null)
    : null;
  const activeThreadTitle =
    threads.find((thread) => thread.id === activeThreadId)?.title ??
    threads[0]?.title ??
    "New chat";
  const effectiveThreadId = activeThreadId ?? threads[0]?.id ?? null;
  const activeChat = selectedGuruId
    ? chat.chatRegistry.get(
        selectedGuruId,
        effectiveThreadId ?? EMPTY_CHAT_THREAD_ID,
      )
    : undefined;
  const workspaceThreadId =
    effectiveThreadId ?? (activeChat ? EMPTY_CHAT_THREAD_ID : null);
  const effectiveChatKey =
    selectedGuruId && workspaceThreadId
      ? chatSessionKey(selectedGuruId, workspaceThreadId)
      : null;
  const workspaceSession = effectiveChatKey
    ? (workspace.sessions[effectiveChatKey] ?? emptyWorkspaceSession())
    : emptyWorkspaceSession();
  const recoveredChat =
    selectedGuruId && effectiveThreadId
      ? chat.runRegistry.getRecoveredChat(selectedGuruId, effectiveThreadId)
      : undefined;
  const setVisibleThreads = useCallback<Dispatch<SetStateAction<ChatThread[]>>>(
    (action) => {
      if (selectedGuruId) chat.setThreadsForGuru(selectedGuruId, action);
    },
    [selectedGuruId, chat.setThreadsForGuru],
  );
  const setVisibleActiveThreadId = useCallback(
    (threadId: string) => {
      if (selectedGuruId) chat.setActiveThreadForGuru(selectedGuruId, threadId);
    },
    [selectedGuruId, chat.setActiveThreadForGuru],
  );
  const abortVisibleRecoveredChat = useCallback(() => {
    if (selectedGuruId && effectiveThreadId) {
      chat.runRegistry.abortRecoveredChat(selectedGuruId, effectiveThreadId);
    }
  }, [effectiveThreadId, chat.runRegistry, selectedGuruId]);
  const runningThreadKeys = useMemo(() => {
    const active = new Set(Object.keys(chat.chatStatuses));
    for (const recovered of chat.runRegistrySnapshot.active_chat_threads) {
      active.add(chatSessionKey(recovered.guru_id, recovered.thread_id));
    }
    return active;
  }, [chat.chatStatuses, chat.runRegistrySnapshot.active_chat_threads]);
  const runningGuruIds = useMemo(() => {
    const active = new Set(chat.runRegistrySnapshot.active_guru_ids);
    for (const candidate of gurus) {
      if (
        (chat.threadsByGuru[candidate.id] ?? []).some((thread) =>
          runningThreadKeys.has(chatSessionKey(candidate.id, thread.id)),
        )
      ) {
        active.add(candidate.id);
      }
    }
    return active;
  }, [
    gurus,
    chat.runRegistrySnapshot.active_guru_ids,
    runningThreadKeys,
    chat.threadsByGuru,
  ]);
  workspace.visibleRef.current = {
    guruId: selectedGuruId,
    threadId: effectiveThreadId,
    tab,
  };
  const chatWorkspaceActions = useMemo<ChatWorkspaceActions>(
    () => ({
      openMemory: (recordId, title) => {
        if (selectedGuruId && workspaceThreadId) {
          workspace.openMemory(selectedGuruId, workspaceThreadId, recordId, title);
        }
      },
      openInLibrary: (recordId) => {
        setRequestedMemory({ record_id: recordId });
        setTab("library");
      },
      openArtifact: (threadId, artifact) => {
        if (selectedGuruId) {
          workspace.openArtifact(selectedGuruId, threadId, artifact);
        }
      },
      openLink: (url) => {
        if (selectedGuruId) {
          workspace.openBrowser(
            selectedGuruId,
            workspaceThreadId ?? EMPTY_CHAT_THREAD_ID,
            url,
          );
        }
      },
    }),
    [
      selectedGuruId,
      workspaceThreadId,
      workspace.openArtifact,
      workspace.openBrowser,
      workspace.openMemory,
    ],
  );
  return (
    <TooltipProvider>
      <SidebarProvider
        className="app-shell"
        style={accentStyle}
        data-theme={resolvedTheme}
      >
        <MacTitlebarControls />
        <AppNavigation
          tab={tab}
          gurus={gurus}
          selectedGuru={selectedGuru}
          loading={guru.loading}
          threads={threads}
          activeThreadId={activeThreadId}
          runningGuruIds={runningGuruIds}
          runningThreadKeys={runningThreadKeys}
          settingsSection={settingsSection}
          updateAvailable={Boolean(appUpdate.status?.offer)}
          onTabChange={(nextTab) => {
            if (nextTab === "settings" && tab !== "settings") {
              settingsReturnTabRef.current = tab;
            }
            setTab(nextTab);
          }}
          onSettingsSectionChange={setSettingsSection}
          onExitSettings={() => setTab(settingsReturnTabRef.current)}
          onSelectGuru={(guruId) =>
            void guru.selectGuru(
              guruId,
              gurus.find((candidate) => candidate.id === guruId),
            )
          }
          onCreateGuru={() => {
            guru.setMutationError(null);
            dialogs.openCreateGuru();
          }}
          onCreateThread={(guruId) => {
            setTab("chat");
            void threadActions.createThreadForGuru(guruId);
          }}
          onRenameThread={dialogs.openRenameThread}
          onDeleteThread={dialogs.setThreadToDelete}
          onSelectThread={(threadId) => {
            if (selectedGuruId) {
              chat.setActiveThreadForGuru(selectedGuruId, threadId);
            }
            setTab("chat");
          }}
        />

        <SidebarInset className="app-frame">
          <AppHeader
            tab={tab}
            title={
              tab === "chat"
                ? activeThreadTitle
                : tab === "library"
                  ? "Memory"
                  : tab[0].toUpperCase() + tab.slice(1)
            }
            guru={selectedGuru}
            workspaceOpen={workspace.open}
            onToggleWorkspace={workspace.toggle}
          />

          <div
            className={`app-stage artifact-placement-${workspace.placement}`}
          >
            <div
              className="app-stage-main"
              hidden={tab === "chat" && workspace.open && workspace.maximized}
            >
              {tab === "agents" ? (
                <div className="app-main">
                  <div
                    className="app-panel"
                    id="main-panel-agents"
                    aria-labelledby="main-tab-agents"
                  >
                    <AgentsView
                      bridge={bridge}
                      agents={gurus}
                      selectedAgent={selectedGuru}
                      loading={guru.loading}
                      mutationBusy={guru.mutationBusy}
                      mutationError={guru.mutationError}
                      recoveryBusy={guru.recoveryBusy}
                      recoveryError={guru.recoveryError}
                      onRecover={() => void guru.recoverSelectedGuru()}
                      onSelect={(guruId) =>
                        void guru.selectGuru(
                          guruId,
                          gurus.find((candidate) => candidate.id === guruId),
                        )
                      }
                      onCreate={() => {
                        guru.setMutationError(null);
                        dialogs.openCreateGuru();
                      }}
                      onImport={() => void guru.addGuru("import")}
                      onRename={() => {
                        if (!selectedGuru) return;
                        guru.setMutationError(null);
                        dialogs.openRenameGuru(selectedGuru.name);
                      }}
                      onExport={() => void guru.exportGuru()}
                      onOpenMarketplace={() => setTab("marketplace")}
                      onDelete={guru.deleteGuru}
                      onAgentUpdated={guru.updateGuru}
                    />
                  </div>
                </div>
              ) : tab === "settings" ? (
                <div className="app-main">
                  <div
                    className="app-panel"
                    id="main-panel-settings"
                    aria-labelledby="main-tab-settings"
                  >
                    <SettingsView
                      section={settingsSection}
                      catalog={model.catalog}
                      theme={theme}
                      loadError={model.settingsError}
                      updateResult={appUpdate.status}
                      updatePhase={appUpdate.phase}
                      updateError={appUpdate.error}
                      onThemeChange={setTheme}
                      onLoadModels={async (provider) => {
                        const next = await bridge.providerModels(provider);
                        model.setCatalog(next);
                        return next;
                      }}
                      onConnect={async (provider, observer) => {
                        const next = await bridge.providerConnect(
                          provider,
                          observer,
                        );
                        model.setCatalog(next);
                        return next;
                      }}
                      onCancelConnect={() => bridge.providerConnectCancel()}
                      onOpenConnectBrowser={() =>
                        bridge.providerConnectOpenBrowser()
                      }
                      onConfigure={model.configureProvider}
                      onUpdateModelVisibility={model.updateModelVisibility}
                      onDisconnect={model.disconnectProvider}
                      onCheckForUpdates={appUpdate.checkForUpdates}
                      onInstallUpdate={(offerId) =>
                        appUpdate.installUpdate({ offer_id: offerId })
                      }
                    />
                  </div>
                </div>
              ) : tab === "marketplace" ? (
                <div className="app-main">
                  <div
                    className="app-panel"
                    id="main-panel-marketplace"
                    aria-labelledby="main-tab-marketplace"
                  >
                    <MarketplaceView bridge={bridge} />
                  </div>
                </div>
              ) : guru.loading || !model.resolved ? (
                <main className="app-loading" aria-label="Opening Guru Terminal">
                  <Spinner className="size-6" />
                  <strong>Opening</strong>
                </main>
              ) : guru.error && !selectedGuru ? (
                <main className="fatal-error" role="alert">
                  <Icon name="close" />
                  <h1>Could not open Guru Terminal</h1>
                  <p>{guru.error}</p>
                  <button
                    type="button"
                    className="primary-button"
                    onClick={() => window.location.reload()}
                  >
                    Try again
                  </button>
                </main>
              ) : selectedGuru?.availability.status === "recovery_required" ? (
                <GuruAvailabilityBoundary
                  availability={selectedGuru.availability}
                  busy={guru.recoveryBusy}
                  error={guru.recoveryError}
                  onRecover={() => void guru.recoverSelectedGuru()}
                />
              ) : tab === "chat" && model.needsProviderSetup ? (
                <main className="guru-onboarding">
                  <h1>Connect a model provider</h1>
                  <p>
                    You pay the provider. Guru Terminal never sees the key.
                  </p>
                  <div className="onboarding-actions">
                    <button
                      type="button"
                      className="primary-button"
                      onClick={() => {
                        settingsReturnTabRef.current = "chat";
                        setSettingsSection("model");
                        setTab("settings");
                      }}
                    >
                      Open Settings
                    </button>
                  </div>
                </main>
              ) : tab === "chat" && model.needsVisibleModels ? (
                <main className="guru-onboarding">
                  <h1>No models in Chat</h1>
                  <p>
                    Every model is hidden. Show at least one in Settings to
                    send a message.
                  </p>
                  <div className="onboarding-actions">
                    <button
                      type="button"
                      className="primary-button"
                      onClick={() => {
                        settingsReturnTabRef.current = "chat";
                        setSettingsSection("model");
                        setTab("settings");
                      }}
                    >
                      Open Settings
                    </button>
                  </div>
                </main>
              ) : !selectedGuru ? (
                <main className="guru-onboarding">
                  <h1>Create a Guru</h1>
                  <p>Name a strategy and start researching.</p>
                  <div className="onboarding-actions">
                    <button
                      type="button"
                      className="primary-button"
                      onClick={() => setTab("agents")}
                    >
                      Create a Guru
                    </button>
                  </div>
                </main>
              ) : selectedGuru ? (
                <main className="app-main">
                  <div
                    className="app-panel"
                    id="main-panel-chat"
                    aria-labelledby="main-tab-chat"
                    aria-hidden={tab !== "chat"}
                    hidden={tab !== "chat"}
                    inert={tab !== "chat"}
                  >
                    {activeChat ? (
                      <ChatView
                        key={selectedGuru.id}
                        bridge={bridge}
                        chat={activeChat}
                        recoveredRun={recoveredChat}
                        guru={selectedGuru}
                        threads={threads}
                        setThreads={setVisibleThreads}
                        activeThreadId={activeThreadId}
                        setActiveThreadId={setVisibleActiveThreadId}
                        onCreateThread={createThread}
                        modelCatalog={model.catalog}
                        modelSelection={modelSelection}
                        onModelSelectionChange={changeSelectedModel}
                        onModelUsed={recordSelectedModel}
                        workspaceActions={chatWorkspaceActions}
                        onAbortRecoveredRun={abortVisibleRecoveredChat}
                      />
                    ) : null}
                  </div>
                  {(tab === "library" ||
                    visitedWorkspaceTabs.has(`${selectedGuru.id}:library`)) && (
                    <div
                      className="app-panel"
                      id="main-panel-library"
                      aria-labelledby="main-tab-library"
                      aria-hidden={tab !== "library"}
                      hidden={tab !== "library"}
                      inert={tab !== "library"}
                    >
                      <LibraryView
                        key={selectedGuru.id}
                        bridge={bridge}
                        guru={selectedGuru}
                        requestedMemory={requestedMemory}
                        refreshToken={selectedGuru.record_count}
                        onRequestConsumed={requestConsumed}
                        onTeachInChat={() => setTab("chat")}
                      />
                    </div>
                  )}
                </main>
              ) : null}
            </div>
            {tab === "chat" &&
              selectedGuru?.availability.status === "available" &&
              workspaceThreadId && (
                <ChatWorkspacePanel
                  bridge={bridge}
                  guruId={selectedGuru.id}
                  threadId={workspaceThreadId}
                  canLoadArtifacts={Boolean(effectiveThreadId)}
                  open={workspace.open}
                  session={workspaceSession}
                  theme={resolvedTheme}
                  width={workspace.panelWidth}
                  height={workspace.panelHeight}
                  placement={workspace.placement}
                  maximized={workspace.maximized}
                  onWidthChange={workspace.setPanelWidth}
                  onHeightChange={workspace.setPanelHeight}
                  onPlacementChange={(placement) => {
                    workspace.setPlacement(placement);
                    workspace.setMaximized(false);
                  }}
                  onMaximizedChange={workspace.setMaximized}
                  onSelectTab={(tabId) => {
                    if (effectiveChatKey) workspace.selectTab(effectiveChatKey, tabId);
                  }}
                  onUpdateTab={(tabId, update) => {
                    if (effectiveChatKey) {
                      workspace.updateTab(effectiveChatKey, tabId, update);
                    }
                  }}
                  onCloseTab={(workspaceTab) => {
                    if (effectiveChatKey) {
                      workspace.closeTab(effectiveChatKey, workspaceTab);
                    }
                  }}
                  onOpenArtifact={(artifact) =>
                    workspace.openArtifact(
                      selectedGuru.id,
                      workspaceThreadId,
                      artifact,
                    )
                  }
                  onNewBrowser={() =>
                    workspace.openBrowser(selectedGuru.id, workspaceThreadId)
                  }
                  onOpenLink={(url) =>
                    workspace.openBrowser(selectedGuru.id, workspaceThreadId, url)
                  }
                  onClose={workspace.close}
                />
              )}
          </div>
        </SidebarInset>
        <AppDialogs
          dialogs={dialogs}
          guruMutationBusy={guru.mutationBusy}
          guruMutationError={guru.mutationError}
          threadMutationBusy={threadActions.mutationBusy}
          onCreateGuru={(name) => guru.addGuru("create", name)}
          onRenameGuru={guru.renameGuru}
          onRenameThread={threadActions.renameThread}
          onDeleteThread={threadActions.deleteThread}
        />
      </SidebarProvider>
    </TooltipProvider>
  );
}
