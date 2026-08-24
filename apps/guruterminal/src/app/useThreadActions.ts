import { useCallback, useState } from "react";
import type { ChatThread, GuruTerminalBridge } from "../types";
import { errorMessage } from "../errors";
import { chatSessionKey } from "../chat/sessionRegistry";
import { EMPTY_CHAT_THREAD_ID, emptyChatThread } from "./emptyChat";
import type { ChatSessions } from "./useChatSessions";
import type { GuruDirectory } from "./useGuruDirectory";
import type { WorkspacePanel } from "./useWorkspacePanel";

type ThreadActionDeps = {
  chat: ChatSessions;
  guru: GuruDirectory;
  workspace: WorkspacePanel;
};

/** Chat thread lifecycle: create, rename, and delete. */
export function useThreadActions(
  bridge: GuruTerminalBridge,
  { chat, guru, workspace }: ThreadActionDeps,
) {
  const [mutationBusy, setMutationBusy] = useState(false);
  const {
    chatRegistry,
    setThreadsForGuru,
    setActiveThreadForGuru,
    threadsByGuruRef,
    activeThreadIdsRef,
  } = chat;
  const { desiredGuruIdRef, selectGuru, setError } = guru;
  const { adoptSession, removeSession } = workspace;

  const createThreadForGuru = useCallback(
    async (guruId: string) => {
      if (desiredGuruIdRef.current !== guruId) {
        const selected = await selectGuru(guruId);
        if (!selected) return null;
      }
      setError(null);
      try {
        const created = await bridge.chatCreate({ guru_id: guruId });
        adoptSession(
          chatSessionKey(guruId, EMPTY_CHAT_THREAD_ID),
          chatSessionKey(guruId, created.id),
        );
        chatRegistry.ensure(created);
        setThreadsForGuru(guruId, (current) => [
          created,
          ...current.filter((thread) => thread.id !== created.id),
        ]);
        if (desiredGuruIdRef.current !== guruId) return null;
        setActiveThreadForGuru(guruId, created.id);
        return created;
      } catch (cause) {
        if (desiredGuruIdRef.current === guruId) {
          setError(errorMessage(cause, "Could not create a new chat."));
        }
        return null;
      }
    },
    [
      adoptSession,
      bridge,
      chatRegistry,
      desiredGuruIdRef,
      selectGuru,
      setActiveThreadForGuru,
      setError,
      setThreadsForGuru,
    ],
  );

  const renameThread = async (target: ChatThread, title: string) => {
    if (!title.trim() || mutationBusy) return false;
    setMutationBusy(true);
    setError(null);
    try {
      const renamed = await bridge.chatRename({
        guru_id: target.guru_id,
        thread_id: target.id,
        title: title.trim(),
      });
      setThreadsForGuru(target.guru_id, (current) =>
        current.map((thread) =>
          thread.id === renamed.id
            ? { ...renamed, messages: thread.messages }
            : thread,
        ),
      );
      return true;
    } catch (cause) {
      if (desiredGuruIdRef.current === target.guru_id) {
        setError(errorMessage(cause, "Could not rename this session."));
      }
      return false;
    } finally {
      setMutationBusy(false);
    }
  };

  const deleteThread = async (thread: ChatThread) => {
    if (mutationBusy) return false;
    setMutationBusy(true);
    setError(null);
    try {
      await bridge.chatDelete({
        guru_id: thread.guru_id,
        thread_id: thread.id,
      });
      const remaining = (threadsByGuruRef.current[thread.guru_id] ?? []).filter(
        (item) => item.id !== thread.id,
      );
      chatRegistry.remove(thread.guru_id, thread.id);
      if (remaining.length === 0) {
        chatRegistry.ensure(emptyChatThread(thread.guru_id));
      }
      setThreadsForGuru(thread.guru_id, remaining);
      if (activeThreadIdsRef.current[thread.guru_id] === thread.id) {
        setActiveThreadForGuru(thread.guru_id, remaining[0]?.id ?? null);
      }
      await removeSession(thread.guru_id, thread.id);
      return true;
    } catch (cause) {
      if (desiredGuruIdRef.current === thread.guru_id) {
        setError(errorMessage(cause, "Could not delete this session."));
      }
      return false;
    } finally {
      setMutationBusy(false);
    }
  };

  return { mutationBusy, createThreadForGuru, renameThread, deleteThread };
}
