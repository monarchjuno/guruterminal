import { Chat } from "@ai-sdk/react";
import type { ChatStatus } from "ai";
import type {
  ChatArtifactRef,
  ChatThread,
  GuruTerminalBridge,
} from "../types";
import {
  fromGuruUIMessage,
  TauriChatTransport,
  toGuruUIMessage,
  type GuruUIMessage,
} from "./ai-sdk";

export type ChatSessionHandlers = {
  onArtifact: (
    guruId: string,
    threadId: string,
    artifact: ChatArtifactRef,
  ) => void;
  onMessages: (
    guruId: string,
    threadId: string,
    messages: ChatThread["messages"],
  ) => void;
  onStatus: (
    guruId: string,
    threadId: string,
    status: ChatStatus,
  ) => void;
  onTitle: (guruId: string, threadId: string, title: string) => void;
};

type RegistryEntry = {
  chat: Chat<GuruUIMessage>;
  guruId: string;
  threadId: string;
  unsubscribeStatus: () => void;
};

export const chatSessionKey = (guruId: string, threadId: string) =>
  `${encodeURIComponent(guruId)}:${encodeURIComponent(threadId)}`;

export const isActiveChatStatus = (status: ChatStatus | undefined) =>
  status === "submitted" || status === "streaming";

/**
 * Owns the long-lived AI SDK Chat object for every Guru/thread pair.
 *
 * React views may mount and unmount as the user changes workspaces. The Chat
 * objects stay here so those view changes never become execution lifecycle
 * changes. Rust remains the authority for persistence and cancellation.
 */
export class ChatSessionRegistry {
  readonly #entries = new Map<string, RegistryEntry>();
  readonly #transport: TauriChatTransport;
  #disposed = false;
  #lifecycleGeneration = 0;

  constructor(
    bridge: GuruTerminalBridge,
    private readonly handlers: ChatSessionHandlers,
  ) {
    this.#transport = new TauriChatTransport(
      bridge,
      (guruId, threadId, title) => {
        if (!this.#disposed) {
          this.handlers.onTitle(guruId, threadId, title);
        }
      },
      (guruId, threadId, artifact) => {
        if (!this.#disposed) {
          this.handlers.onArtifact(guruId, threadId, artifact);
        }
      },
    );
  }

  ensure(thread: ChatThread) {
    const key = chatSessionKey(thread.guru_id, thread.id);
    const existing = this.#entries.get(key);
    if (existing) return existing.chat;

    const guruId = thread.guru_id;
    const threadId = thread.id;
    const chat = new Chat<GuruUIMessage>({
      id: key,
      messages: thread.messages.map(toGuruUIMessage),
      transport: this.#transport,
      onError: () => {
        if (!this.#disposed) {
          this.handlers.onStatus(guruId, threadId, chat.status);
        }
      },
      onFinish: ({ messages }) => {
        if (!this.#disposed) {
          this.handlers.onMessages(
            guruId,
            threadId,
            messages.map(fromGuruUIMessage),
          );
        }
      },
    });
    const unsubscribeStatus = chat["~registerStatusCallback"](() => {
      if (!this.#disposed) {
        this.handlers.onStatus(guruId, threadId, chat.status);
      }
    });
    this.#entries.set(key, {
      chat,
      guruId,
      threadId,
      unsubscribeStatus,
    });
    this.handlers.onStatus(guruId, threadId, chat.status);
    return chat;
  }

  get(guruId: string, threadId: string) {
    return this.#entries.get(chatSessionKey(guruId, threadId))?.chat;
  }

  isActive(guruId: string, threadId: string) {
    return isActiveChatStatus(this.get(guruId, threadId)?.status);
  }

  activate() {
    this.#lifecycleGeneration += 1;
    this.#disposed = false;
  }

  /**
   * React StrictMode performs a setup → cleanup → setup probe. Deferring the
   * destructive detach by one microtask lets the second setup cancel that
   * probe while a real unmount still releases every status subscription.
   */
  deactivate() {
    const generation = ++this.#lifecycleGeneration;
    queueMicrotask(() => {
      if (generation === this.#lifecycleGeneration) this.dispose();
    });
  }

  /**
   * Refresh an idle Chat from canonical SQLite data. A live Chat is never
   * replaced; its in-flight stream remains the freshest local projection.
   */
  reconcile(thread: ChatThread) {
    const chat = this.ensure(thread);
    const localMessages = chat.messages.map(fromGuruUIMessage);
    if (isActiveChatStatus(chat.status)) {
      return {
        ...thread,
        messages: localMessages,
      };
    }

    // A Guru selection may have started before a just-completed response was
    // committed. Do not let that older snapshot overwrite the finished Chat.
    if (localMessages.length > thread.messages.length) {
      return {
        ...thread,
        messages: localMessages,
      };
    }

    chat.messages = thread.messages.map(toGuruUIMessage);
    return thread;
  }

  remove(guruId: string, threadId: string) {
    const key = chatSessionKey(guruId, threadId);
    const entry = this.#entries.get(key);
    if (!entry) return;
    entry.unsubscribeStatus();
    this.#entries.delete(key);
    this.handlers.onStatus(guruId, threadId, "ready");
  }

  removeGuru(guruId: string) {
    for (const entry of [...this.#entries.values()]) {
      if (entry.guruId === guruId) this.remove(guruId, entry.threadId);
    }
  }

  dispose() {
    this.#lifecycleGeneration += 1;
    this.#disposed = true;
    for (const entry of this.#entries.values()) entry.unsubscribeStatus();
    this.#entries.clear();
  }
}
