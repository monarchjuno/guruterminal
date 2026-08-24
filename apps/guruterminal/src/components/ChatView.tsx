import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";
import type { Chat } from "@ai-sdk/react";
import type { FileUIPart } from "ai";
import { useGuruChat } from "../chat/useGuruChat";
import {
  withVisibleSteers,
  type VisibleSteer,
} from "../chat/visibleSteers";
import type { GuruUIMessage } from "../chat/ai-sdk";
import type {
  ChatArtifactRef,
  ChatMessage,
  ChatThread,
  GuruTerminalBridge,
  GuruSummary,
  ModelCatalog,
  ModelRunSelection,
} from "../types";
import { visibleCatalogModels } from "../modelSelection";
import type { RecoveredChatRun } from "../run/guruRunRegistry";
import {
  ChatComposer,
  type ComposerPluginOption,
  type ComposerSkillOption,
} from "./chat/ChatComposer";
import { ChatConversation } from "./chat/ChatConversation";
import {
  ChatPendingQueue,
  type QueuedChatMessage,
} from "./chat/ChatPendingQueue";
import {
  emptyChatSetupSources,
  isComposerMentionPlugin,
  shouldShowEmptySetup,
  type EmptySetupSource,
} from "../app/chatOnboarding";
import { promptSelectsMemorySkill } from "../chat/memorySkillSelection";
import { errorMessage } from "../errors";

/** Workspace-launch callbacks already bound to the visible guru and thread. */
export type ChatWorkspaceActions = {
  openMemory: (recordId: string, title: string) => void;
  openInLibrary: (recordId: string) => void;
  openArtifact: (threadId: string, artifact: ChatArtifactRef) => void;
  openLink: (url: string) => void;
};

type Props = {
  bridge: GuruTerminalBridge;
  chat: Chat<GuruUIMessage>;
  recoveredRun?: RecoveredChatRun;
  guru: GuruSummary;
  threads: ChatThread[];
  setThreads: Dispatch<SetStateAction<ChatThread[]>>;
  activeThreadId: string | null;
  setActiveThreadId: (id: string) => void;
  onCreateThread: () => Promise<ChatThread | null>;
  workspaceActions: ChatWorkspaceActions;
  modelCatalog: ModelCatalog | null;
  modelSelection: ModelRunSelection;
  onModelSelectionChange: (selection: ModelRunSelection) => void;
  onModelUsed: (selection: ModelRunSelection) => void;
  onAbortRecoveredRun: () => void;
  onOpenMarketplace: () => void;
};

export function ChatView({
  bridge,
  chat,
  recoveredRun,
  guru,
  threads,
  setThreads,
  activeThreadId,
  setActiveThreadId,
  onCreateThread,
  workspaceActions,
  modelCatalog,
  modelSelection,
  onModelSelectionChange,
  onModelUsed,
  onAbortRecoveredRun,
  onOpenMarketplace,
}: Props) {
  const [prompt, setPrompt] = useState("");
  const [useMemory, setUseMemory] = useState(true);
  const [updateMemory, setUpdateMemory] = useState(true);
  const memoryBeforeLock = useRef<{
    useMemory: boolean;
    updateMemory: boolean;
  } | null>(null);
  const [composerSkills, setComposerSkills] = useState<ComposerSkillOption[]>(
    [],
  );
  const [composerPlugins, setComposerPlugins] = useState<
    ComposerPluginOption[]
  >([]);
  const [setupSources, setSetupSources] = useState<EmptySetupSource[]>([]);
  const [setupEpoch, setSetupEpoch] = useState(0);
  const [setupBusy, setSetupBusy] = useState(false);
  const [setupError, setSetupError] = useState<string | null>(null);
  const [queuedByThread, setQueuedByThread] = useState<
    Record<string, QueuedChatMessage[]>
  >({});
  const [steersByThread, setSteersByThread] = useState<
    Record<string, VisibleSteer[]>
  >({});
  const [queueHoldByThread, setQueueHoldByThread] = useState<
    Record<string, string | null>
  >({});
  const consumeThreadRef = useRef<string | null>(null);
  const messagesRef = useRef<ChatMessage[]>([]);
  const steersRef = useRef(steersByThread);
  const queuedRef = useRef(queuedByThread);
  const [pendingSubmission, setPendingSubmission] = useState<{
    threadId: string;
    text: string;
    useMemory: boolean;
    updateMemory: boolean;
    modelProfileId: string;
    thinkingLevel: string;
    runOptions: Record<string, string>;
    files: FileUIPart[];
  } | null>(null);
  const selectedModelProfileId = modelSelection.model_profile_id;
  const selectedThinkingLevel = modelSelection.thinking_level;
  const selectedRunOptions = modelSelection.run_options;
  const memorySkillSelected = promptSelectsMemorySkill(prompt);
  const memorySkillSelectedRef = useRef(memorySkillSelected);
  memorySkillSelectedRef.current = memorySkillSelected;

  const changePrompt = useCallback(
    (nextPrompt: string) => {
      const nextSelected = promptSelectsMemorySkill(nextPrompt);
      if (!memorySkillSelected && nextSelected) {
        memoryBeforeLock.current = { useMemory, updateMemory };
        setUseMemory(true);
        setUpdateMemory(true);
      } else if (memorySkillSelected && !nextSelected) {
        const previous = memoryBeforeLock.current;
        if (previous) {
          setUseMemory(previous.useMemory);
          setUpdateMemory(previous.updateMemory);
        }
        memoryBeforeLock.current = null;
      }
      setPrompt(nextPrompt);
    },
    [memorySkillSelected, updateMemory, useMemory],
  );

  const activeThread = useMemo<ChatThread | undefined>(
    () => threads.find((thread) => thread.id === activeThreadId) ?? threads[0],
    [activeThreadId, threads],
  );
  const readAttachment = useCallback(
    async (messageId: string, attachmentId: string) => {
      if (!activeThread?.id) throw new Error("Chat thread is unavailable.");
      const result = await bridge.chatAttachmentRead(
        guru.id,
        activeThread.id,
        messageId,
        attachmentId,
      );
      return result.data_url;
    },
    [activeThread?.id, bridge, guru.id],
  );
  const {
    messages,
    messageKeys,
    isRunning: localIsRunning,
    isStopping: localIsStopping,
    announcement: localStreamAnnouncement,
    error: localError,
    submit,
    steer,
    commitSteers,
    abort,
  } = useGuruChat({
    bridge,
    chat,
    guruId: guru.id,
    activeThread,
    setThreads,
  });
  messagesRef.current = messages;
  steersRef.current = steersByThread;
  queuedRef.current = queuedByThread;
  const activeRecoveredRun = localIsRunning ? undefined : recoveredRun;
  const isRunning = localIsRunning || Boolean(activeRecoveredRun);
  const openChatArtifact = useCallback(
    (artifact: ChatArtifactRef) => {
      if (activeThread?.id) {
        workspaceActions.openArtifact(activeThread.id, artifact);
      }
    },
    [activeThread?.id, workspaceActions],
  );
  const openChatLink = useCallback(
    async (url: string) => {
      workspaceActions.openLink(url);
    },
    [workspaceActions],
  );
  const revertMemory = useCallback(
    async (recordId: string, commitId: string) => {
      await bridge.libraryMemoryRevert({
        guru_id: guru.id,
        record_id: recordId,
        commit_id: commitId,
      });
    },
    [bridge, guru.id],
  );
  const streamAnnouncement = activeRecoveredRun
    ? activeRecoveredRun.abort_requested
      ? "Stopping the recovered response."
      : activeRecoveredRun.status === "reconciling"
        ? "Refreshing the completed response."
        : "Guru is continuing a response started before reload."
    : localStreamAnnouncement;
  const error = activeRecoveredRun?.error ?? localError;
  const abortActiveRun = useCallback(() => {
    if (localIsRunning) void abort();
    else onAbortRecoveredRun();
  }, [abort, localIsRunning, onAbortRecoveredRun]);

  useEffect(() => {
    if (!activeThreadId && threads[0]) setActiveThreadId(threads[0].id);
  }, [activeThreadId, setActiveThreadId, threads]);

  useEffect(() => {
    const nextUseMemory = activeThread?.use_memory ?? true;
    const nextUpdateMemory = activeThread?.update_memory ?? true;
    if (memorySkillSelectedRef.current) {
      memoryBeforeLock.current = {
        useMemory: nextUseMemory,
        updateMemory: nextUpdateMemory,
      };
      setUseMemory(true);
      setUpdateMemory(true);
    } else {
      setUseMemory(nextUseMemory);
      setUpdateMemory(nextUpdateMemory);
    }
  }, [activeThread?.id, activeThread?.update_memory, activeThread?.use_memory]);

  useEffect(() => {
    let cancelled = false;
    void Promise.all([
      bridge.agentSkillCatalog(guru.id),
      bridge.guruCapabilityList(guru.id),
      bridge.marketplaceSnapshot(),
    ])
      .then(([skills, bindings, marketplace]) => {
        if (cancelled) return;
        setComposerSkills(
          skills
            .filter((skill) => skill.enabled)
            .map((skill) => ({
              id: skill.id,
              name: skill.name,
              description: skill.description,
            })),
        );
        const entriesById = new Map(
          marketplace.catalog.entries.map((entry) => [entry.id, entry]),
        );
        setComposerPlugins(
          bindings.flatMap((binding) => {
            if (!binding.enabled) return [];
            const entry = entriesById.get(binding.entry_id);
            if (!entry || !isComposerMentionPlugin(entry)) return [];
            return [
              {
                id: binding.entry_id,
                name: entry.name,
                description: entry.summary,
              },
            ];
          }),
        );
        const sources = emptyChatSetupSources(marketplace, bindings);
        setSetupSources(shouldShowEmptySetup(sources) ? sources : []);
      })
      .catch(() => {
        if (cancelled) return;
        setComposerSkills([]);
        setComposerPlugins([]);
        setSetupSources([]);
      });
    return () => {
      cancelled = true;
    };
  }, [bridge, guru.enabled_skill_ids, guru.id, setupEpoch]);

  const configureSetupSource = useCallback(
    async (entryId: string, config: Record<string, string>) => {
      setSetupBusy(true);
      setSetupError(null);
      try {
        await bridge.marketplaceConnectorConfigure({
          entry_id: entryId,
          config,
        });
        await bridge.guruCapabilityEnable({
          guru_id: guru.id,
          entry_id: entryId,
        });
        setSetupEpoch((current) => current + 1);
      } catch (cause: unknown) {
        setSetupError(
          errorMessage(cause, "Could not save the contact email."),
        );
      } finally {
        setSetupBusy(false);
      }
    },
    [bridge, guru.id],
  );

  const enableSetupSource = useCallback(
    async (entryId: string) => {
      setSetupBusy(true);
      setSetupError(null);
      try {
        await bridge.guruCapabilityEnable({
          guru_id: guru.id,
          entry_id: entryId,
        });
        setSetupEpoch((current) => current + 1);
      } catch (cause: unknown) {
        setSetupError(
          errorMessage(cause, "Could not enable this source for the Guru."),
        );
      } finally {
        setSetupBusy(false);
      }
    },
    [bridge, guru.id],
  );

  useEffect(() => {
    if (
      !pendingSubmission ||
      activeThread?.id !== pendingSubmission.threadId ||
      isRunning
    ) {
      return;
    }
    const submission = pendingSubmission;
    setPendingSubmission(null);
    onModelUsed({
      model_profile_id: submission.modelProfileId,
      thinking_level: submission.thinkingLevel,
      run_options: submission.runOptions,
    });
    void submit(
      submission.threadId,
      submission.text,
      submission.useMemory,
      submission.updateMemory,
      submission.modelProfileId,
      submission.thinkingLevel,
      submission.runOptions,
      submission.files,
    );
  }, [activeThread?.id, isRunning, onModelUsed, pendingSubmission, submit]);

  const send = async (submittedPrompt?: string, files: FileUIPart[] = []) => {
    const text = (submittedPrompt ?? prompt).trim();
    if ((!text && files.length === 0) || !selectedModelProfileId) return;

    if (isRunning) {
      if (files.length > 0 || !text || !activeThread?.id) return;
      const receipt = await steer(text);
      if (receipt) {
        const threadId = activeThread.id;
        commitSteers([receipt]);
        setSteersByThread((current) => ({
          ...current,
          [threadId]: [...(current[threadId] ?? []), receipt],
        }));
        changePrompt("");
      }
      return;
    }

    let targetThread = activeThread;
    if (!targetThread) targetThread = (await onCreateThread()) ?? undefined;
    if (!targetThread) return;
    changePrompt("");
    if (activeThread?.id !== targetThread.id) {
      setPendingSubmission({
        threadId: targetThread.id,
        text,
        useMemory,
        updateMemory,
        modelProfileId: selectedModelProfileId,
        thinkingLevel: selectedThinkingLevel,
        runOptions: selectedRunOptions,
        files,
      });
      return;
    }

    onModelUsed({
      model_profile_id: selectedModelProfileId,
      thinking_level: selectedThinkingLevel,
      run_options: selectedRunOptions,
    });
    await submit(
      targetThread.id,
      text,
      useMemory,
      updateMemory,
      selectedModelProfileId,
      selectedThinkingLevel,
      selectedRunOptions,
      files,
    );
  };

  const enqueueFollowUp = () => {
    const text = prompt.trim();
    if (!isRunning || !text || !activeThread?.id) return;
    const threadId = activeThread.id;
    setQueuedByThread((current) => ({
      ...current,
      [threadId]: [
        ...(current[threadId] ?? []),
        {
          id: crypto.randomUUID(),
          text,
          createdAt: new Date().toISOString(),
        },
      ],
    }));
    setQueueHoldByThread((current) => ({ ...current, [threadId]: null }));
    changePrompt("");
  };

  const updateQueued = (
    threadId: string,
    updater: (items: QueuedChatMessage[]) => QueuedChatMessage[],
  ) => {
    setQueuedByThread((current) => ({
      ...current,
      [threadId]: updater(current[threadId] ?? []),
    }));
  };

  const sendQueuedNow = (id: string) => {
    if (!activeThread?.id || !selectedModelProfileId) return;
    const threadId = activeThread.id;
    const item = (queuedByThread[threadId] ?? []).find(
      (queued) => queued.id === id,
    );
    if (!item || isRunning) return;
    updateQueued(threadId, (items) =>
      items.filter((queued) => queued.id !== id),
    );
    onModelUsed({
      model_profile_id: selectedModelProfileId,
      thinking_level: selectedThinkingLevel,
      run_options: selectedRunOptions,
    });
    void submit(
      threadId,
      item.text,
      useMemory,
      updateMemory,
      selectedModelProfileId,
      selectedThinkingLevel,
      selectedRunOptions,
      [],
    );
  };

  useEffect(() => {
    if (isRunning && activeThread?.id) {
      consumeThreadRef.current = activeThread.id;
    }
  }, [activeThread?.id, isRunning]);

  useEffect(() => {
    if (isRunning) return;
    const threadId = consumeThreadRef.current;
    if (!threadId || threadId !== activeThread?.id || !selectedModelProfileId) {
      return;
    }
    const lastStatus = messagesRef.current.at(-1)?.status;
    if (
      lastStatus !== "complete" &&
      lastStatus !== "aborted" &&
      lastStatus !== "error"
    ) {
      return;
    }
    consumeThreadRef.current = null;
    const delivered = steersRef.current[threadId] ?? [];
    commitSteers(delivered);
    if (lastStatus === "aborted") {
      setQueueHoldByThread((current) => ({
        ...current,
        [threadId]: "Response stopped. Queued messages were kept.",
      }));
      return;
    }
    if (lastStatus === "error") {
      setQueueHoldByThread((current) => ({
        ...current,
        [threadId]: "Response failed. Queued messages were kept.",
      }));
      return;
    }
    if (lastStatus !== "complete") return;
    setQueueHoldByThread((current) => ({ ...current, [threadId]: null }));
    const [next, ...rest] = queuedRef.current[threadId] ?? [];
    setQueuedByThread((current) => ({ ...current, [threadId]: rest }));
    if (!next) return;
    setPendingSubmission({
      threadId,
      text: next.text,
      useMemory,
      updateMemory,
      modelProfileId: selectedModelProfileId,
      thinkingLevel: selectedThinkingLevel,
      runOptions: selectedRunOptions,
      files: [],
    });
  }, [
    activeThread?.id,
    commitSteers,
    isRunning,
    selectedModelProfileId,
    selectedRunOptions,
    selectedThinkingLevel,
    updateMemory,
    useMemory,
    messages,
  ]);

  useEffect(() => {
    if (isRunning || !activeThread?.id) return;
    const threadId = activeThread.id;
    const delivered = steersByThread[threadId] ?? [];
    if (delivered.length === 0) return;
    const present = new Set(messages.map((message) => message.id));
    const remaining = delivered.filter((item) => !present.has(item.id));
    if (remaining.length === delivered.length) return;
    setSteersByThread((current) => ({ ...current, [threadId]: remaining }));
  }, [activeThread?.id, isRunning, messages, steersByThread]);

  const activeSteers = activeThread
    ? steersByThread[activeThread.id]
    : undefined;
  const visibleConversation = useMemo(
    () => withVisibleSteers(messages, messageKeys, activeSteers ?? []),
    [activeSteers, messageKeys, messages],
  );

  return (
    <section className="chat-layout" aria-label="Chat with Guru">
      <div className="chat-workspace">
        <header
          className={
            localIsStopping || activeRecoveredRun
              ? "workspace-heading is-status"
              : "workspace-heading"
          }
        >
          <h1>{activeThread?.title ?? "New chat"}</h1>
          {localIsStopping || activeRecoveredRun ? (
            <div className="thread-statuses">
              {localIsStopping ? (
                <div className="memory-state" role="status">
                  <span className="status-dot on" />
                  Stopping
                </div>
              ) : null}
              {activeRecoveredRun ? (
                <div className="memory-state" role="status">
                  <span className="status-dot on" />
                  {activeRecoveredRun.abort_requested
                    ? "Stopping"
                    : activeRecoveredRun.status === "reconciling"
                      ? "Updating the answer"
                      : "Still working"}
                </div>
              ) : null}
            </div>
          ) : null}
        </header>

        <ChatConversation
          messages={visibleConversation.messages}
          messageKeys={visibleConversation.messageKeys}
          guruName={guru.name}
          setupSources={setupSources}
          setupBusy={setupBusy}
          setupError={setupError}
          onSuggestion={changePrompt}
          onOpenMarketplace={onOpenMarketplace}
          onConfigureSource={configureSetupSource}
          onEnableSource={enableSetupSource}
          onOpenMemory={workspaceActions.openMemory}
          onOpenInLibrary={workspaceActions.openInLibrary}
          onOpenArtifact={openChatArtifact}
          onOpenLink={openChatLink}
          onReadAttachment={readAttachment}
          onRevertMemory={revertMemory}
        />

        {activeThread && (
          <ChatPendingQueue
            queued={queuedByThread[activeThread.id] ?? []}
            holdReason={queueHoldByThread[activeThread.id]}
            onRemove={(id) =>
              updateQueued(activeThread.id, (items) =>
                items.filter((item) => item.id !== id),
              )
            }
            onEdit={(id, text) =>
              updateQueued(activeThread.id, (items) =>
                items.map((item) => (item.id === id ? { ...item, text } : item)),
              )
            }
            onMove={(id, offset) =>
              updateQueued(activeThread.id, (items) => {
                const index = items.findIndex((item) => item.id === id);
                const nextIndex = index + offset;
                if (index < 0 || nextIndex < 0 || nextIndex >= items.length) {
                  return items;
                }
                const next = [...items];
                const [moved] = next.splice(index, 1);
                next.splice(nextIndex, 0, moved!);
                return next;
              })
            }
            onSendNow={sendQueuedNow}
            canSendNow={!isRunning}
          />
        )}

        <ChatComposer
          prompt={prompt}
          useMemory={useMemory}
          updateMemory={updateMemory}
          memoryControlsLocked={memorySkillSelected}
          isRunning={isRunning}
          isStopping={
            localIsStopping || Boolean(activeRecoveredRun?.abort_requested)
          }
          error={error}
          streamAnnouncement={streamAnnouncement}
          models={visibleCatalogModels(modelCatalog)}
          providers={modelCatalog?.providers ?? []}
          modelSelection={modelSelection}
          skills={composerSkills}
          plugins={composerPlugins}
          onPromptChange={changePrompt}
          onUseMemoryChange={setUseMemory}
          onUpdateMemoryChange={setUpdateMemory}
          onModelSelectionChange={onModelSelectionChange}
          onSend={(text, files) => void send(text, files)}
          onFollowUp={enqueueFollowUp}
          onAbort={abortActiveRun}
        />
      </div>
    </section>
  );
}
