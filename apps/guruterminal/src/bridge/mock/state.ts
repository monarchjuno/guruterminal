import { MOCK_GURUS, MOCK_LIBRARY, MOCK_THREADS } from "../../mockData";
import type {
  AgentSkillSummary,
  ChatArtifact,
  ChartDataset,
  ChatArtifactRevision,
  ChatThread,
  GuruSummary,
  LibraryRecord,
  RunActivity,
} from "../../types";

export const DEFAULT_AGENT_SKILL_IDS = ["research", "wiki", "lens", "decision"] as const;

export type MockUserSkill = AgentSkillSummary & {
  content: string;
  revision: number;
};

export type MockBridgeState = {
  delay_ms: number;
  gurus: GuruSummary[];
  threads: Record<string, ChatThread[]>;
  library: Record<string, LibraryRecord[]>;
  memoryPrevious: Record<string, Record<string, string>>;
  user_skills: Record<string, MockUserSkill[]>;
  chat_runs: Map<string, AbortController>;
  artifacts: Record<
    string,
    Array<{ artifact: ChatArtifact; content: ChatArtifactRevision; dataset?: ChartDataset }>
  >;
  run_activities: Map<string, RunActivity>;
};

export const clone = <T>(value: T): T => structuredClone(value);

let nextId = 1;

export const makeId = (prefix: string) =>
  `${prefix}-${Date.now()}-${nextId++}`;

export const createMockBridgeState = (delay_ms: number): MockBridgeState => ({
  delay_ms,
  gurus: clone(MOCK_GURUS),
  threads: clone(MOCK_THREADS),
  library: clone(MOCK_LIBRARY),
  memoryPrevious: {},
  user_skills: Object.fromEntries(MOCK_GURUS.map((guru) => [guru.id, []])),
  chat_runs: new Map(),
  artifacts: {},
  run_activities: new Map(),
});

export const registerRunActivity = (
  state: MockBridgeState,
  activity: RunActivity,
) => {
  if (state.run_activities.has(activity.run_id)) {
    throw new Error("A run with the same ID is already active.");
  }
  const modelKinds = new Set<RunActivity["kind"]>(["chat"]);
  if (
    modelKinds.has(activity.kind) &&
    [...state.run_activities.values()].filter((current) =>
      modelKinds.has(current.kind),
    ).length >= 4
  ) {
    throw new Error("All four model worker slots are active.");
  }
  const targetDomain = (kind: RunActivity["kind"]) =>
    kind === "chat" || kind === "chat_mutation"
      ? "chat"
      : kind;
  const targetConflict = [...state.run_activities.values()].some(
    (current) =>
      current.guru_id === activity.guru_id &&
      targetDomain(current.kind) === targetDomain(activity.kind) &&
      current.target === activity.target,
  );
  if (targetConflict) {
    throw new Error("This exact Guru run target is already active.");
  }
  const guruAccessConflict = [...state.run_activities.values()].some(
    (current) =>
      current.guru_id === activity.guru_id &&
      (current.kind === "memory_write" || activity.kind === "memory_write"),
  );
  if (guruAccessConflict) {
    throw new Error("This Guru has an active Memory reader or writer.");
  }
  state.run_activities.set(activity.run_id, activity);
};

export const ensureGuruHasNoActiveRuns = (
  state: MockBridgeState,
  guruId: string,
) => {
  if (
    [...state.run_activities.values()].some(
      (activity) => activity.guru_id === guruId,
    )
  ) {
    throw new Error("This Guru still has active sessions.");
  }
};

export const finishRunActivity = (state: MockBridgeState, runId: string) => {
  state.run_activities.delete(runId);
};

export const wait = (milliseconds: number, signal: AbortSignal) =>
  new Promise<void>((resolve, reject) => {
    if (signal.aborted) {
      reject(new DOMException("Aborted", "AbortError"));
      return;
    }

    const timer = window.setTimeout(resolve, milliseconds);
    signal.addEventListener(
      "abort",
      () => {
        window.clearTimeout(timer);
        reject(new DOMException("Aborted", "AbortError"));
      },
      { once: true },
    );
  });

export const shortPause = async (state: MockBridgeState) => {
  if (state.delay_ms === 0) return;
  await new Promise<void>((resolve) =>
    window.setTimeout(resolve, state.delay_ms),
  );
};
