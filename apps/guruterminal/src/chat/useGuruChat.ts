import { useChat, type Chat } from "@ai-sdk/react";
import type { FileUIPart } from "ai";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";
import type { ChatThread, GuruTerminalBridge } from "../types";
import { errorMessage } from "../errors";
import {
  fromGuruUIMessage,
  toGuruUIMessage,
  type GuruUIMessage,
} from "./ai-sdk";

type Options = {
  bridge: GuruTerminalBridge;
  chat: Chat<GuruUIMessage>;
  guruId: string;
  activeThread: ChatThread | undefined;
  setThreads: Dispatch<SetStateAction<ChatThread[]>>;
};

// Tauri Channels may drain a burst of native token events synchronously. Keep
// progressive rendering responsive while yielding between React store updates.
const STREAM_RENDER_THROTTLE_MS = 50;

const markLatestAssistantAborted = (messages: GuruUIMessage[]) => {
  let assistantIndex = messages.length - 1;
  while (
    assistantIndex >= 0 &&
    messages[assistantIndex]?.role !== "assistant"
  ) {
    assistantIndex -= 1;
  }
  if (assistantIndex < 0) return messages;

  return messages.map((message, index) => {
    if (index !== assistantIndex) return message;
    const hasText = message.parts.some(
      (part) => part.type === "text" && part.text.length > 0,
    );
    return {
      ...message,
      metadata: { ...message.metadata, status: "aborted" as const },
      parts: hasText
        ? message.parts.map((part) =>
            part.type === "text" ? { ...part, state: "done" as const } : part,
          )
        : [
            ...message.parts,
            {
              type: "text" as const,
              text: "Response stopped.",
              state: "done" as const,
            },
          ],
    };
  });
};

const sameThreadMessages = (
  current: ChatThread["messages"],
  next: ChatThread["messages"],
) =>
  current.length === next.length &&
  current.every(
    (message, index) => JSON.stringify(message) === JSON.stringify(next[index]),
  );

export function useGuruChat({
  bridge,
  chat,
  guruId,
  activeThread,
  setThreads,
}: Options) {
  const [announcement, setAnnouncement] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [isStopping, setIsStopping] = useState(false);
  const mountedRef = useRef(true);
  const {
    messages: uiMessages,
    sendMessage,
    setMessages,
    stop,
    status,
    error: chatError,
  } = useChat<GuruUIMessage>({
    chat,
    throttle: STREAM_RENDER_THROTTLE_MS,
  });

  const messages = useMemo(
    () => uiMessages.map(fromGuruUIMessage),
    [uiMessages],
  );
  const messageKeys = useMemo(
    () => uiMessages.map((message) => message.id),
    [uiMessages],
  );
  const isRunning =
    status === "submitted" || status === "streaming" || isStopping;
  const lastMessageStatus = messages.at(-1)?.status;
  const activeProgress = [...(messages.at(-1)?.progress?.items ?? [])]
    .reverse()
    .find((item) => item.kind !== "commentary" && item.status === "running");
  const visibleAnnouncement = isStopping
    ? "Stopping response."
    : isRunning
      ? activeProgress && activeProgress.kind !== "commentary"
        ? activeProgress.action
        : "Guru is generating a response."
      : lastMessageStatus === "aborted"
        ? "Response stopped."
        : lastMessageStatus === "error" || chatError
          ? "Response failed."
          : lastMessageStatus === "complete"
            ? "Response complete."
            : announcement;

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    if (!activeThread?.id || isRunning) return;
    const threadId = activeThread.id;
    setThreads((current) => {
      const thread = current.find((candidate) => candidate.id === threadId);
      if (!thread || sameThreadMessages(thread.messages, messages)) {
        return current;
      }
      return current.map((candidate) =>
        candidate.id === threadId ? { ...candidate, messages } : candidate,
      );
    });
  }, [activeThread?.id, isRunning, messages, setThreads]);

  const submit = useCallback(
    async (
      threadId: string,
      text: string,
      useMemory: boolean,
      updateMemory: boolean,
      modelProfileId: string,
      thinkingLevel: string,
      runOptions: Record<string, string>,
      files: FileUIPart[],
    ) => {
      const now = new Date().toISOString();
      setThreads((current) =>
        current.map((thread) =>
          thread.id === threadId
            ? {
                ...thread,
                use_memory: useMemory,
                update_memory: updateMemory,
                updated_at: now,
              }
            : thread,
        ),
      );
      setError(null);
      setAnnouncement("Guru is generating a response.");
      try {
        await sendMessage(
          {
            text,
            files,
            metadata: { created_at: now, status: "complete" },
          },
          {
            body: {
              guru_id: guruId,
              thread_id: threadId,
              use_memory: useMemory,
              update_memory: updateMemory,
              model_profile_id: modelProfileId,
              thinking_level: thinkingLevel,
              run_options: runOptions,
            },
          },
        );
      } catch (cause) {
        if (!mountedRef.current) return;
        setError(errorMessage(cause, "Could not start the response."));
      }
    },
    [guruId, sendMessage, setThreads],
  );

  const commitSteers = useCallback(
    (steers: Array<{ id: string; text: string; createdAt: string }>) => {
      if (steers.length === 0) return;
      setMessages((current) => {
        const existing = new Set(
          current.map((message) => message.metadata?.native_message_id ?? message.id),
        );
        const incoming = steers
          .filter((item) => !existing.has(item.id))
          .map((item) =>
            toGuruUIMessage({
              id: item.id,
              role: "user",
              content: item.text,
              created_at: item.createdAt,
              status: "complete",
            }),
          );
        if (incoming.length === 0) return current;
        const insertAt =
          current.at(-1)?.role === "assistant"
            ? current.length - 1
            : current.length;
        return [
          ...current.slice(0, insertAt),
          ...incoming,
          ...current.slice(insertAt),
        ];
      });
    },
    [setMessages],
  );

  const steer = useCallback(
    async (text: string) => {
      if (!activeThread?.id) return null;
      setError(null);
      try {
        const receipt = await bridge.chatSteer({
          guru_id: guruId,
          thread_id: activeThread.id,
          prompt: text,
        });
        setAnnouncement("Guidance sent to the active response.");
        return {
          id: receipt.message_id,
          text: receipt.prompt,
          createdAt: receipt.created_at,
        };
      } catch (cause) {
        setError(errorMessage(cause, "Could not steer the active response."));
        return null;
      }
    },
    [activeThread?.id, bridge, guruId],
  );

  const abort = useCallback(async () => {
    if (!isRunning || isStopping || !activeThread?.id) return;
    const threadId = activeThread.id;
    const expectedMessageCount = uiMessages.length;
    setIsStopping(true);
    try {
      await stop();
      const deadline = Date.now() + 10_000;
      while (Date.now() < deadline) {
        const active = (await bridge.runActivityList()).some(
          (run) =>
            run.kind === "chat" &&
            run.guru_id === guruId &&
            run.target === threadId,
        );
        if (!active) {
          const workspace = await bridge.guruSelect(guruId);
          const canonical = workspace.threads.find(
            (thread) => thread.id === threadId,
          );
          const terminal = canonical?.messages.at(-1);
          if (
            canonical &&
            canonical.messages.length >= expectedMessageCount &&
            terminal?.role === "assistant" &&
            ["complete", "aborted", "error"].includes(
              terminal.status ?? "complete",
            )
          ) {
            setMessages(canonical.messages.map(toGuruUIMessage));
            return;
          }
        }
        await new Promise((resolve) => window.setTimeout(resolve, 50));
      }
      throw new Error(
        "Native Chat did not finish saving the stopped response.",
      );
    } catch (cause) {
      setMessages(markLatestAssistantAborted);
      setError(errorMessage(cause, "Could not stop the response."));
    } finally {
      setIsStopping(false);
    }
  }, [
    activeThread?.id,
    bridge,
    guruId,
    isRunning,
    isStopping,
    setMessages,
    stop,
    uiMessages.length,
  ]);

  return {
    messages,
    messageKeys,
    isRunning,
    isStopping,
    announcement: visibleAnnouncement,
    error: chatError?.message ?? error,
    submit,
    steer,
    commitSteers,
    abort,
  };
}
