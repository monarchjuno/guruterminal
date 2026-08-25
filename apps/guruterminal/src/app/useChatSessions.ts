import { useCallback, useEffect, useRef, useState, type SetStateAction } from "react";
import type {
  ChatArtifactRef,
  ChatThread,
  GuruTerminalBridge,
  GuruSummary,
  GuruWorkspace,
} from "../types";
import type { ChatStatus } from "ai";
import {
  ChatSessionRegistry,
  chatSessionKey,
  isActiveChatStatus,
} from "../chat/sessionRegistry";
import {
  GuruRunRegistry,
  useGuruRunRegistrySnapshot,
} from "../run/guruRunRegistry";

/** Cross-domain callbacks wired by the composition root after every hook exists. */
export type ChatSessionHandlers = {
  publishArtifact: (
    guruId: string,
    threadId: string,
    artifact: ChatArtifactRef,
  ) => void;
  refreshGuruAvailability: (guruId: string) => Promise<GuruSummary | null>;
  applyRecoveredGuru: (guru: GuruSummary) => void;
};

/** Per-guru chat threads, live statuses, and the chat/run registries. */
export function useChatSessions(bridge: GuruTerminalBridge) {
  const [threadsByGuru, setThreadsByGuru] = useState<
    Record<string, ChatThread[]>
  >({});
  const [activeThreadIds, setActiveThreadIds] = useState<
    Record<string, string | null>
  >({});
  const [chatStatuses, setChatStatuses] = useState<Record<string, ChatStatus>>(
    {},
  );
  const threadsByGuruRef = useRef(threadsByGuru);
  const activeThreadIdsRef = useRef(activeThreadIds);
  const chatRegistryRef = useRef<ChatSessionRegistry | null>(null);
  const runRegistryRef = useRef<GuruRunRegistry | null>(null);
  const handlersRef = useRef<ChatSessionHandlers | null>(null);

  const setThreadsForGuru = useCallback(
    (guruId: string, action: SetStateAction<ChatThread[]>) => {
      const current = threadsByGuruRef.current[guruId] ?? [];
      const nextThreads =
        typeof action === "function" ? action(current) : action;
      if (nextThreads === current) return;
      const next = {
        ...threadsByGuruRef.current,
        [guruId]: nextThreads,
      };
      threadsByGuruRef.current = next;
      setThreadsByGuru(next);
    },
    [],
  );

  const setActiveThreadForGuru = useCallback(
    (guruId: string, threadId: string | null) => {
      const next = {
        ...activeThreadIdsRef.current,
        [guruId]: threadId,
      };
      activeThreadIdsRef.current = next;
      setActiveThreadIds(next);
    },
    [],
  );

  const updateChatTitle = useCallback(
    (guruId: string, threadId: string, title: string) => {
      setThreadsForGuru(guruId, (current) =>
        current.map((thread) =>
          thread.id === threadId ? { ...thread, title } : thread,
        ),
      );
    },
    [setThreadsForGuru],
  );

  const updateChatMessages = useCallback(
    (guruId: string, threadId: string, messages: ChatThread["messages"]) => {
      if (messages.at(-1)?.memory_update) {
        void handlersRef.current
          ?.refreshGuruAvailability(guruId)
          .catch(() => undefined);
      }
      const updatedAt = messages.at(-1)?.created_at;
      setThreadsForGuru(guruId, (current) =>
        current.map((thread) =>
          thread.id === threadId
            ? {
                ...thread,
                messages,
                updated_at: updatedAt ?? thread.updated_at,
              }
            : thread,
        ),
      );
    },
    [setThreadsForGuru],
  );

  const updateChatStatus = useCallback(
    (guruId: string, threadId: string, status: ChatStatus) => {
      const key = chatSessionKey(guruId, threadId);
      if (isActiveChatStatus(status)) {
        runRegistryRef.current?.claimLocalChat(guruId, threadId);
      }
      setChatStatuses((current) => {
        if (isActiveChatStatus(status)) {
          if (current[key] === status) return current;
          return { ...current, [key]: status };
        }
        if (!(key in current)) return current;
        const next = { ...current };
        delete next[key];
        return next;
      });
    },
    [],
  );

  const publishChatArtifact = useCallback(
    (guruId: string, threadId: string, artifact: ChatArtifactRef) => {
      handlersRef.current?.publishArtifact(guruId, threadId, artifact);
    },
    [],
  );

  const reconcileRecoveredChat = useCallback(
    (workspace: GuruWorkspace, threadId: string) => {
      const canonical = workspace.threads.find(
        (thread) => thread.id === threadId,
      );
      if (!canonical) return;
      const reconciled =
        chatRegistryRef.current?.reconcile(canonical) ?? canonical;
      setThreadsForGuru(workspace.guru.id, (current) =>
        current.some((thread) => thread.id === threadId)
          ? current.map((thread) =>
              thread.id === threadId ? reconciled : thread,
            )
          : [reconciled, ...current],
      );
      if (!activeThreadIdsRef.current[workspace.guru.id]) {
        setActiveThreadForGuru(workspace.guru.id, threadId);
      }
      handlersRef.current?.applyRecoveredGuru(workspace.guru);
    },
    [setActiveThreadForGuru, setThreadsForGuru],
  );

  const [chatRegistry] = useState(
    () =>
      new ChatSessionRegistry(bridge, {
        onArtifact: publishChatArtifact,
        onMessages: updateChatMessages,
        onStatus: updateChatStatus,
        onTitle: updateChatTitle,
      }),
  );
  chatRegistryRef.current = chatRegistry;
  const [runRegistry] = useState(
    () =>
      new GuruRunRegistry(bridge, {
        isLocalChatActive: (guruId, threadId) =>
          chatRegistryRef.current?.isActive(guruId, threadId) ?? false,
        onRecoveredChatReconciled: reconcileRecoveredChat,
      }),
  );
  runRegistryRef.current = runRegistry;
  const runRegistrySnapshot = useGuruRunRegistrySnapshot(runRegistry);

  useEffect(() => {
    chatRegistry.activate();
    return () => chatRegistry.deactivate();
  }, [chatRegistry]);

  useEffect(() => {
    runRegistry.activate();
    return () => runRegistry.deactivate();
  }, [runRegistry]);

  /** Drops every session and registry entry owned by one guru. */
  const removeGuru = useCallback(
    (guruId: string) => {
      chatRegistry.removeGuru(guruId);
      runRegistry.removeGuru(guruId);
      const nextThreadsByGuru = { ...threadsByGuruRef.current };
      delete nextThreadsByGuru[guruId];
      threadsByGuruRef.current = nextThreadsByGuru;
      setThreadsByGuru(nextThreadsByGuru);
      const nextActiveThreadIds = { ...activeThreadIdsRef.current };
      delete nextActiveThreadIds[guruId];
      activeThreadIdsRef.current = nextActiveThreadIds;
      setActiveThreadIds(nextActiveThreadIds);
    },
    [chatRegistry, runRegistry],
  );

  return {
    threadsByGuru,
    activeThreadIds,
    chatStatuses,
    threadsByGuruRef,
    activeThreadIdsRef,
    setThreadsForGuru,
    setActiveThreadForGuru,
    chatRegistry,
    runRegistry,
    runRegistrySnapshot,
    handlersRef,
    removeGuru,
  };
}

export type ChatSessions = ReturnType<typeof useChatSessions>;
