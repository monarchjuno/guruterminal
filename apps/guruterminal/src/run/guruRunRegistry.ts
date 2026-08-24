import { useSyncExternalStore } from "react";
import { errorMessage } from "../errors";
import type { GuruTerminalBridge, GuruWorkspace, RunActivity } from "../types";

export type RecoveredChatRun = {
  run_id: string;
  guru_id: string;
  thread_id: string;
  status: "running" | "reconciling";
  abort_requested: boolean;
  error?: string;
};

type Handlers = {
  isLocalChatActive?: (guruId: string, threadId: string) => boolean;
  onRecoveredChatReconciled?: (
    workspace: GuruWorkspace,
    threadId: string,
  ) => void | Promise<void>;
};

type GuruRunSnapshot = {
  chats: Readonly<Record<string, RecoveredChatRun>>;
  hydration: "idle" | "loading" | "ready" | "error";
};

export type GuruRunRegistrySnapshot = {
  version: number;
  active_guru_ids: ReadonlySet<string>;
  active_chat_threads: readonly { guru_id: string; thread_id: string }[];
};

const EMPTY: GuruRunSnapshot = { chats: {}, hydration: "idle" };
const timerKey = (guruId: string, threadId: string) =>
  JSON.stringify([guruId, threadId]);

/** Keeps native Chat runs addressable while the user navigates or reloads. */
export class GuruRunRegistry {
  readonly #states = new Map<string, GuruRunSnapshot>();
  readonly #listeners = new Set<() => void>();
  readonly #hydrations = new Map<string, Promise<void>>();
  readonly #monitors = new Map<string, number>();
  readonly #abortRetries = new Map<string, number>();
  readonly #epochs = new Map<string, number>();
  #disposed = false;
  #generation = 0;
  #snapshot: GuruRunRegistrySnapshot = {
    version: 0,
    active_guru_ids: new Set(),
    active_chat_threads: [],
  };

  constructor(
    private readonly bridge: GuruTerminalBridge,
    private readonly handlers: Handlers = {},
  ) {}

  readonly subscribe = (listener: () => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };
  readonly getSnapshot = () => this.#snapshot;
  getGuruSnapshot(guruId: string) {
    return this.#states.get(guruId) ?? EMPTY;
  }
  getRecoveredChat(guruId: string, threadId: string) {
    return this.getGuruSnapshot(guruId).chats[threadId];
  }

  activate() {
    this.#generation += 1;
    this.#disposed = false;
  }
  deactivate() {
    const generation = ++this.#generation;
    queueMicrotask(() => {
      if (generation === this.#generation) this.dispose();
    });
  }
  dispose() {
    this.#generation += 1;
    this.#disposed = true;
    for (const timer of this.#monitors.values()) window.clearTimeout(timer);
    for (const timer of this.#abortRetries.values()) window.clearTimeout(timer);
    this.#monitors.clear();
    this.#abortRetries.clear();
    this.#listeners.clear();
  }
  removeGuru(guruId: string) {
    for (const threadId of Object.keys(this.getGuruSnapshot(guruId).chats))
      this.#clearTimers(guruId, threadId);
    this.#epochs.set(guruId, (this.#epochs.get(guruId) ?? 0) + 1);
    this.#hydrations.delete(guruId);
    if (this.#states.delete(guruId)) this.#emit();
  }

  hydrateGuru(guruId: string) {
    const existing = this.#hydrations.get(guruId);
    if (existing) return existing;
    this.#update(guruId, (state) => ({ ...state, hydration: "loading" }));
    const epoch = this.#epochs.get(guruId) ?? 0;
    const hydration = this.#loadActivities(guruId, epoch)
      .then(() => {
        if (!this.#disposed && (this.#epochs.get(guruId) ?? 0) === epoch)
          this.#update(guruId, (state) => ({ ...state, hydration: "ready" }));
      })
      .catch(() => {
        if (!this.#disposed && (this.#epochs.get(guruId) ?? 0) === epoch)
          this.#update(guruId, (state) => ({ ...state, hydration: "error" }));
      })
      .finally(() => {
        if (this.#hydrations.get(guruId) === hydration)
          this.#hydrations.delete(guruId);
      });
    this.#hydrations.set(guruId, hydration);
    return hydration;
  }
  async hydrateActivities(guruIds: readonly string[]) {
    const allowed = new Set(guruIds);
    try {
      for (const activity of await this.bridge.runActivityList())
        if (allowed.has(activity.guru_id) && activity.kind === "chat")
          this.#attach(activity);
    } catch {
      // Activity discovery is best-effort; canonical Chat remains in SQLite.
    }
  }
  async #loadActivities(guruId: string, epoch: number) {
    const activities = await this.bridge.runActivityList();
    if (this.#disposed || (this.#epochs.get(guruId) ?? 0) !== epoch) return;
    for (const activity of activities)
      if (activity.guru_id === guruId && activity.kind === "chat")
        this.#attach(activity);
  }

  claimLocalChat(guruId: string, threadId: string) {
    const run = this.getRecoveredChat(guruId, threadId);
    if (!run) return false;
    this.#remove(guruId, threadId, run.run_id);
    return true;
  }
  abortRecoveredChat(guruId: string, threadId: string) {
    const run = this.getRecoveredChat(guruId, threadId);
    if (!run || run.abort_requested) return false;
    this.#updateChat(guruId, threadId, (current) => ({
      ...current,
      abort_requested: true,
      error: undefined,
    }));
    this.#pollAbort(guruId, threadId, run.run_id, 0);
    return true;
  }

  #attach(activity: RunActivity) {
    const guruId = activity.guru_id;
    const threadId = activity.target;
    if (this.handlers.isLocalChatActive?.(guruId, threadId)) {
      this.claimLocalChat(guruId, threadId);
      return;
    }
    const previous = this.getRecoveredChat(guruId, threadId);
    if (previous?.run_id === activity.run_id) {
      const key = timerKey(guruId, threadId);
      if (!this.#monitors.has(key))
        this.#monitor(guruId, threadId, activity.run_id);
      return;
    }
    if (previous) this.#clearTimers(guruId, threadId);
    this.#update(guruId, (state) => ({
      ...state,
      chats: {
        ...state.chats,
        [threadId]: {
          run_id: activity.run_id,
          guru_id: guruId,
          thread_id: threadId,
          status: "running",
          abort_requested: false,
        },
      },
    }));
    this.#monitor(guruId, threadId, activity.run_id);
  }

  #monitor(guruId: string, threadId: string, runId: string) {
    const key = timerKey(guruId, threadId);
    this.#clear(this.#monitors, key);
    const poll = async () => {
      if (this.#disposed || !this.#isRun(guruId, threadId, runId)) return;
      try {
        const active = (await this.bridge.runActivityList()).some(
          (activity) =>
            activity.kind === "chat" &&
            activity.run_id === runId &&
            activity.guru_id === guruId &&
            activity.target === threadId,
        );
        if (!this.#isRun(guruId, threadId, runId)) return;
        if (active) {
          this.#updateChat(guruId, threadId, (current) => ({
            ...current,
            status: "running",
            error: undefined,
          }));
        } else {
          this.#updateChat(guruId, threadId, (current) => ({
            ...current,
            status: "reconciling",
            error: undefined,
          }));
          try {
            const workspace = await this.bridge.guruSelect(guruId);
            if (!this.#isRun(guruId, threadId, runId)) return;
            await this.handlers.onRecoveredChatReconciled?.(workspace, threadId);
            this.#remove(guruId, threadId, runId);
            return;
          } catch (cause) {
            if (this.#isRun(guruId, threadId, runId))
              this.#updateChat(guruId, threadId, (current) => ({
                ...current,
                status: "reconciling",
                error: errorMessage(
                  cause,
                  "The response finished, but its canonical Chat could not be refreshed yet.",
                ),
              }));
          }
        }
      } catch {
        // Keep exact run identity locked until Rust confirms completion.
      }
      if (!this.#disposed && this.#isRun(guruId, threadId, runId))
        this.#monitors.set(key, window.setTimeout(poll, 400));
    };
    this.#monitors.set(key, window.setTimeout(poll, 0));
  }

  #pollAbort(guruId: string, threadId: string, runId: string, attempt: number) {
    const key = timerKey(guruId, threadId);
    this.#clear(this.#abortRetries, key);
    void this.bridge.chatAbort(runId).catch((cause) => {
      const run = this.getRecoveredChat(guruId, threadId);
      if (this.#disposed || run?.run_id !== runId || !run.abort_requested) return;
      if (attempt >= 49) {
        this.#updateChat(guruId, threadId, (current) => ({
          ...current,
          abort_requested: false,
          error: errorMessage(cause, "Could not stop the recovered response."),
        }));
      } else {
        this.#abortRetries.set(
          key,
          window.setTimeout(
            () => this.#pollAbort(guruId, threadId, runId, attempt + 1),
            100,
          ),
        );
      }
    });
  }

  #isRun(guruId: string, threadId: string, runId: string) {
    return this.getRecoveredChat(guruId, threadId)?.run_id === runId;
  }
  #updateChat(
    guruId: string,
    threadId: string,
    updater: (current: RecoveredChatRun) => RecoveredChatRun,
  ) {
    const current = this.getRecoveredChat(guruId, threadId);
    if (!current) return;
    this.#update(guruId, (state) => ({
      ...state,
      chats: { ...state.chats, [threadId]: updater(current) },
    }));
  }
  #remove(guruId: string, threadId: string, runId: string) {
    if (!this.#isRun(guruId, threadId, runId)) return;
    this.#clearTimers(guruId, threadId);
    this.#update(guruId, (state) => {
      const chats = { ...state.chats };
      delete chats[threadId];
      return { ...state, chats };
    });
  }
  #update(guruId: string, updater: (state: GuruRunSnapshot) => GuruRunSnapshot) {
    this.#states.set(guruId, updater(this.getGuruSnapshot(guruId)));
    this.#emit();
  }
  #emit() {
    const activeGuruIds = new Set<string>();
    const activeChatThreads: Array<{ guru_id: string; thread_id: string }> = [];
    for (const [guruId, state] of this.#states) {
      for (const threadId of Object.keys(state.chats))
        activeChatThreads.push({ guru_id: guruId, thread_id: threadId });
      if (Object.keys(state.chats).length > 0) activeGuruIds.add(guruId);
    }
    this.#snapshot = {
      version: this.#snapshot.version + 1,
      active_guru_ids: activeGuruIds,
      active_chat_threads: activeChatThreads,
    };
    if (!this.#disposed)
      for (const listener of this.#listeners) listener();
  }
  #clear(timers: Map<string, number>, key: string) {
    const timer = timers.get(key);
    if (timer !== undefined) window.clearTimeout(timer);
    timers.delete(key);
  }
  #clearTimers(guruId: string, threadId: string) {
    const key = timerKey(guruId, threadId);
    this.#clear(this.#monitors, key);
    this.#clear(this.#abortRetries, key);
  }
}

export const useGuruRunRegistrySnapshot = (registry: GuruRunRegistry) =>
  useSyncExternalStore(
    registry.subscribe,
    registry.getSnapshot,
    registry.getSnapshot,
  );
