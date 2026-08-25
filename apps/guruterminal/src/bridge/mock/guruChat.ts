import type {
  AgentHarnessSnapshot,
  AgentSkillSummary,
  AgentSkillsUpdateRequest,
  ChatArtifact,
  ChartDataset,
  ChatArtifactRef,
  ChatArtifactRevision,
  ChatArtifactView,
  ChatCreateRequest,
  ChatControlReceipt,
  ChatControlRequest,
  ChatDeleteRequest,
  ChatMessage,
  ChatProgress,
  ChatRenameRequest,
  ChatSendRequest,
  ChatStreamEvent,
  ChatThread,
  ExecutionModelLock,
  GuruCreateRequest,
  GuruDeleteRequest,
  GuruRenameRequest,
  GuruSummary,
  GuruWorkspace,
  MemoryRef,
  MemoryUpdateResult,
  StreamObserver,
} from "../../types";
import {
  DEFAULT_AGENT_SKILL_IDS,
  clone,
  ensureGuruHasNoActiveRuns,
  finishRunActivity,
  makeId,
  registerRunActivity,
  shortPause,
  wait,
  type MockBridgeState,
} from "./state";

const AGENT_SKILLS: ReadonlyArray<Omit<AgentSkillSummary, "enabled">> = [
  {
    id: "research",
    name: "Research",
    description:
      "Use when the answer depends on financial facts, valuation, comparison, a thesis challenge, a market move, or a simple historical rule test.",
    ownership: "bundled",
    editable: false,
  },
  {
    id: "wiki",
    name: "Wiki",
    description:
      "Use when the user selects this Skill or asks to organize, update, or retain stable descriptive facts in Wiki.",
    ownership: "bundled",
    editable: false,
  },
  {
    id: "lens",
    name: "Lens",
    description:
      "Use when the user selects this Skill or asks to review a Decision, update an interpretive lens, or record a falsifiable hypothesis.",
    ownership: "bundled",
    editable: false,
  },
];

const selectableSkillIds = new Set<string>(DEFAULT_AGENT_SKILL_IDS);

export const guruList = async (
  state: MockBridgeState,
): Promise<GuruSummary[]> => clone(state.gurus);

export const guruSelect = async (
  state: MockBridgeState,
  guru_id: string,
): Promise<GuruWorkspace> => {
  const guru = state.gurus.find((item) => item.id === guru_id);
  if (!guru) throw new Error("Guru not found.");
  return {
    guru: clone(guru),
    threads: clone(state.threads[guru_id] ?? []),
  };
};

export const agentSkillCatalog = async (
  state: MockBridgeState,
  guru_id: string,
): Promise<AgentSkillSummary[]> => {
  const guru = state.gurus.find((item) => item.id === guru_id);
  if (!guru) throw new Error("Guru not found.");
  return [
    ...AGENT_SKILLS.map((skill) => ({
      ...skill,
      enabled: guru.enabled_skill_ids.includes(skill.id),
    })),
    ...(state.user_skills[guru_id] ?? []).map(
      ({ content: _, revision: __, ...skill }) => skill,
    ),
  ];
};

export const agentSkillsUpdate = async (
  state: MockBridgeState,
  request: AgentSkillsUpdateRequest,
): Promise<GuruSummary> => {
  const guru = state.gurus.find((item) => item.id === request.guru_id);
  if (!guru) throw new Error("Guru not found.");
  if (
    new Set(request.skill_ids).size !== request.skill_ids.length ||
    request.skill_ids.some(
      (id) =>
        !selectableSkillIds.has(id) &&
        !(state.user_skills[request.guru_id] ?? []).some(
          (skill) => skill.id === id,
        ),
    )
  ) {
    throw new Error("Unknown or duplicate agent skill.");
  }
  const deterministicOrder = [
    ...DEFAULT_AGENT_SKILL_IDS,
    ...(state.user_skills[request.guru_id] ?? []).map((skill) => skill.id),
  ];
  guru.enabled_skill_ids = deterministicOrder.filter((id) =>
    request.skill_ids.includes(id),
  );
  guru.updated_at = new Date().toISOString();
  await shortPause(state);
  return clone(guru);
};

export const guruCreate = async (
  state: MockBridgeState,
  request: GuruCreateRequest,
): Promise<GuruSummary> => {
  const guru: GuruSummary = {
    id: makeId("guru"),
    name: request.name.trim(),
    philosophy:
      "Build your own investment principles through research and reflection.",
    record_count: 0,
    updated_at: new Date().toISOString(),
    accent: "#a4530c",
    enabled_skill_ids: [...DEFAULT_AGENT_SKILL_IDS],
    availability: { status: "available" },
  };
  state.gurus.push(guru);
  state.threads[guru.id] = [];
  state.library[guru.id] = [];
  state.user_skills[guru.id] = [];
  await shortPause(state);
  return clone(guru);
};

export const guruImport = async (
  state: MockBridgeState,
): Promise<GuruSummary> => {
  const guru: GuruSummary = {
    id: makeId("guru-imported"),
    name: "Imported Guru",
    philosophy: "Use the imported project's existing investment memory as-is.",
    record_count: 12,
    updated_at: new Date().toISOString(),
    accent: "#4d5f8e",
    enabled_skill_ids: [...DEFAULT_AGENT_SKILL_IDS],
    availability: { status: "available" },
  };
  state.gurus.push(guru);
  state.threads[guru.id] = [];
  state.library[guru.id] = [];
  state.user_skills[guru.id] = [];
  await shortPause(state);
  return clone(guru);
};

export const guruRename = async (
  state: MockBridgeState,
  request: GuruRenameRequest,
): Promise<GuruSummary> => {
  const guru = state.gurus.find((item) => item.id === request.guru_id);
  if (!guru) throw new Error("Guru not found.");
  guru.name = request.name.trim();
  guru.updated_at = new Date().toISOString();
  await shortPause(state);
  return clone(guru);
};

export const guruDelete = async (
  state: MockBridgeState,
  request: GuruDeleteRequest,
): Promise<void> => {
  ensureGuruHasNoActiveRuns(state, request.guru_id);
  const index = state.gurus.findIndex((item) => item.id === request.guru_id);
  if (index < 0) throw new Error("Guru not found.");
  const threadIds = new Set(
    (state.threads[request.guru_id] ?? []).map((thread) => thread.id),
  );
  state.gurus.splice(index, 1);
  delete state.threads[request.guru_id];
  delete state.library[request.guru_id];
  delete state.user_skills[request.guru_id];
  threadIds.forEach((threadId) => delete state.artifacts[threadId]);
  await shortPause(state);
};

export const chatCreate = async (
  state: MockBridgeState,
  request: ChatCreateRequest,
): Promise<ChatThread> => {
  const thread: ChatThread = {
    id: makeId("thread"),
    guru_id: request.guru_id,
    title: request.title?.trim() || "New chat",
    updated_at: new Date().toISOString(),
    use_memory: true,
    update_memory: true,
    messages: [],
  };
  state.threads[request.guru_id] ??= [];
  state.threads[request.guru_id].unshift(thread);
  return clone(thread);
};

export const chatRename = async (
  state: MockBridgeState,
  request: ChatRenameRequest,
): Promise<ChatThread> => {
  const runId = makeId("chat-mutation");
  registerRunActivity(state, {
    run_id: runId,
    guru_id: request.guru_id,
    kind: "chat_mutation",
    target: request.thread_id,
    started_at_ms: Date.now(),
  });
  try {
    const thread = state.threads[request.guru_id]?.find(
      (item) => item.id === request.thread_id,
    );
    if (!thread) throw new Error("Chat thread not found.");
    const title = request.title.trim();
    if (!title) throw new Error("Chat title is required.");
    await shortPause(state);
    thread.title = title;
    return clone(thread);
  } finally {
    finishRunActivity(state, runId);
  }
};

export const chatDelete = async (
  state: MockBridgeState,
  request: ChatDeleteRequest,
): Promise<void> => {
  const runId = makeId("chat-mutation");
  registerRunActivity(state, {
    run_id: runId,
    guru_id: request.guru_id,
    kind: "chat_mutation",
    target: request.thread_id,
    started_at_ms: Date.now(),
  });
  try {
    const threads = state.threads[request.guru_id] ?? [];
    const index = threads.findIndex((item) => item.id === request.thread_id);
    if (index < 0) throw new Error("Chat thread not found.");
    await shortPause(state);
    threads.splice(index, 1);
    delete state.artifacts[request.thread_id];
  } finally {
    finishRunActivity(state, runId);
  }
};

export const chatAttachmentRead = async (
  state: MockBridgeState,
  guru_id: string,
  thread_id: string,
  message_id: string,
  attachment_id: string,
): Promise<{ data_url: string }> => {
  const message = state.threads[guru_id]
    ?.find((thread) => thread.id === thread_id)
    ?.messages.find((item) => item.id === message_id);
  const attachment = message?.attachments?.find(
    (item) => item.id === attachment_id,
  );
  if (!attachment?.url) throw new Error("Chat attachment not found.");
  return { data_url: attachment.url };
};

export const chatArtifactList = async (
  state: MockBridgeState,
  guru_id: string,
  thread_id: string,
): Promise<ChatArtifact[]> => {
  const thread = state.threads[guru_id]?.find((item) => item.id === thread_id);
  if (!thread) throw new Error("Chat thread not found.");
  return clone(
    (state.artifacts[thread_id] ?? [])
      .map((entry) => entry.artifact)
      .sort((left, right) => right.updated_at_ms - left.updated_at_ms),
  );
};

export const chatArtifactRead = async (
  state: MockBridgeState,
  guru_id: string,
  thread_id: string,
  artifact_id: string,
): Promise<ChatArtifactView> => {
  const thread = state.threads[guru_id]?.find((item) => item.id === thread_id);
  if (!thread) throw new Error("Chat thread not found.");
  const entry = (state.artifacts[thread_id] ?? []).find(
    (item) => item.artifact.id === artifact_id,
  );
  if (!entry) throw new Error("Artifact not found.");
  return clone({
    artifact: entry.artifact,
    revision: entry.content,
    chart_dataset: entry.content.payload.kind === "chart" ? entry.dataset : undefined,
  });
};

const mockArtifactForPrompt = (
  state: MockBridgeState,
  request: ChatSendRequest,
  message_id: string,
  created_at: string,
): ChatArtifactRef | undefined => {
  const entries = (state.artifacts[request.thread_id] ??= []);
  const referencedArtifactId = /@artifact\/([^\s]+)/u.exec(request.prompt)?.[1];
  const referenced = referencedArtifactId
    ? entries.find((entry) => entry.artifact.id === referencedArtifactId)
    : undefined;
  const revising =
    Boolean(referenced) && /\brevise\b|\bupdate\b|수정|추가/iu.test(request.prompt);
  const requestsArtifactOutput =
    /\bartifact\b|\bdocument\b|markdown|문서|\bchart\b|차트/iu.test(
      request.prompt,
    );
  if (!requestsArtifactOutput && !revising) return undefined;

  const wantsChart =
    /\bchart\b|차트/iu.test(request.prompt) ||
    (revising && referenced?.artifact.kind === "chart");
  const wantsMarkdown =
    /\bartifact\b|\bdocument\b|markdown|문서/iu.test(
    request.prompt,
    ) || (revising && referenced?.artifact.kind === "markdown");
  if (!wantsChart && !wantsMarkdown) return undefined;

  const artifact_id = revising ? referenced!.artifact.id : makeId("artifact");
  const revision = revising ? referenced!.artifact.current_revision + 1 : 1;
  const timestamp = Date.parse(created_at);
  const chartDataset: ChartDataset | undefined = wantsChart ? {
    id: makeId("dataset"),
    columns: [
      { id: "date", label: "Date", kind: "date" },
      { id: "open", label: "Open", kind: "number" },
      { id: "high", label: "High", kind: "number" },
      { id: "low", label: "Low", kind: "number" },
      { id: "close", label: "Close", kind: "number" },
      { id: "volume", label: "Volume", kind: "number" },
    ],
    rows: [
      ["2026-08-04", 100, 105, 98, 103, 1200],
      ["2026-08-05", 103, 108, 101, 106, 1800],
      ["2026-08-06", 106, 109, 102, 104, 1500],
    ],
    lineage: { kind: "agent_authored", upstream_receipts: [] },
    digest: "d".repeat(64),
  } : undefined;
  const payload: ChatArtifactRevision["payload"] = wantsChart
    ? {
        kind: "chart",
        schema: "guruterminal-chart/2",
        chart: {
          dataset_id: chartDataset!.id,
          dataset_digest: chartDataset!.digest,
          view: {
            kind: "financial",
            symbol: "MOCK",
            interval: "1d",
            time: "date",
            open: "open",
            high: "high",
            low: "low",
            close: "close",
            volume: "volume",
            price_precision: 2,
          },
          studies: [{ module_id: "VOL", calc_params: [] }],
          drawings: [],
          note: "Mock daily price series generated for the local preview.",
        },
      }
    : {
        kind: "markdown",
        schema: "guruterminal-markdown/1",
        markdown: [
          "# Research note",
          "",
          "## Summary",
          "",
          `This document was created for: **${request.prompt}**`,
          "",
          "- Separate observations from interpretation.",
          "- Keep unresolved claims explicit.",
        ].join("\n"),
      };
  const digest = String(revision).repeat(64).slice(0, 64);
  const artifactRevision: ChatArtifactRevision = {
    artifact_id,
    revision,
    payload,
    digest,
    source_message_id: message_id,
    created_at_ms: timestamp,
  };
  const title = wantsChart ? "Price and volume" : "Research note";
  if (revising) {
    referenced!.artifact = {
      ...referenced!.artifact,
      title,
      current_revision: revision,
      updated_at_ms: timestamp,
    };
    referenced!.content = artifactRevision;
    referenced!.dataset = chartDataset;
  } else {
    entries.push({
      artifact: {
        id: artifact_id,
        chat_session_id: request.thread_id,
        kind: payload.kind,
        title,
        current_revision: revision,
        created_at_ms: timestamp,
        updated_at_ms: timestamp,
      },
      content: artifactRevision,
      dataset: chartDataset,
    });
  }
  return { artifact_id, revision, kind: payload.kind, title, digest };
};

const mockChatTitle = (prompt: string) => {
  const normalized = prompt.trim().replace(/\s+/g, " ");
  return Array.from(normalized).slice(0, 40).join("") || "New chat";
};

export const chatSend = async (
  state: MockBridgeState,
  request: ChatSendRequest,
  observer: StreamObserver<ChatStreamEvent>,
  capability_ids: string[],
  execution_model: ExecutionModelLock,
): Promise<{ run_id: string }> => {
  const run_id = request.run_id;
  const controller = new AbortController();
  registerRunActivity(state, {
    run_id,
    guru_id: request.guru_id,
    kind: "chat",
    target: request.thread_id,
    started_at_ms: Date.now(),
  });
  state.chat_runs.set(run_id, controller);
  window.setTimeout(() => {
    void streamChat(
      state,
      run_id,
      request,
      observer,
      controller,
      capability_ids,
      execution_model,
    );
  }, 0);
  return { run_id };
};

const submitChatControl = async (
  state: MockBridgeState,
  request: ChatControlRequest,
): Promise<ChatControlReceipt> => {
  const active = [...state.run_activities.values()].some(
    (run) =>
      run.kind === "chat" &&
      run.guru_id === request.guru_id &&
      run.target === request.thread_id,
  );
  if (!active) throw new Error("Active Chat run not found.");
  const thread = state.threads[request.guru_id]?.find(
    (item) => item.id === request.thread_id,
  );
  if (!thread) throw new Error("Chat thread not found.");
  const prompt = request.prompt.trim();
  if (!prompt) throw new Error("Prompt is required.");
  const receipt: ChatControlReceipt = {
    message_id: makeId("message"),
    prompt,
    created_at: new Date().toISOString(),
    mode: "steer",
  };
  thread.messages.push({
    id: receipt.message_id,
    role: "user",
    content: prompt,
    created_at: receipt.created_at,
    status: "complete",
  });
  thread.updated_at = receipt.created_at;
  return receipt;
};

export const chatSteer = (
  state: MockBridgeState,
  request: ChatControlRequest,
) => submitChatControl(state, request);

const streamChat = async (
  state: MockBridgeState,
  run_id: string,
  request: ChatSendRequest,
  observer: StreamObserver<ChatStreamEvent>,
  controller: AbortController,
  capability_ids: string[],
  execution_model: ExecutionModelLock,
) => {
  const thread = state.threads[request.guru_id]?.find(
    (item) => item.id === request.thread_id,
  );
  const created_at = new Date().toISOString();
  const message_id = makeId("assistant");
  const guru = state.gurus.find((item) => item.id === request.guru_id);
  const agent_harness: AgentHarnessSnapshot = {
    schema: "guruterminal-harness/1",
    mode: "chat",
    skill_ids: [...(guru?.enabled_skill_ids ?? [])],
    capability_ids: [...capability_ids],
    digest: "e".repeat(64),
  };
  let content = "";
  let memory_refs: MemoryRef[] = [];
  const progress: ChatProgress = {
    startedAtMs: Date.now(),
    items: [],
  };
  const emitProgress = () =>
    observer({ type: "progress", run_id, progress: clone(progress) });
  const setProgress = (
    id: string,
    action: string,
    status: "running" | "succeeded",
  ) => {
    const existing = progress.items.find(
      (item) => item.kind !== "commentary" && item.id === id,
    );
    if (existing && existing.kind !== "commentary") {
      existing.status = status;
      if (status === "succeeded") existing.finishedAtMs = Date.now();
    } else {
      progress.items.push({
        id,
        kind: id === "title" ? "system" : "tool",
        category: id.startsWith("memory-") ? "memory" : "system",
        operation: id.endsWith("search")
          ? "search"
          : id.endsWith("read")
            ? "read"
            : "generic",
        action,
        status,
        startedAtMs: Date.now(),
        finishedAtMs: status === "succeeded" ? Date.now() : undefined,
      });
    }
    emitProgress();
  };

  try {
    observer({ type: "started", run_id });
    if (thread) {
      thread.use_memory = request.use_memory;
      thread.update_memory = request.update_memory;
      thread.messages.push({
        id: makeId("user"),
        role: "user",
        content: request.prompt,
        created_at,
        status: "complete",
        attachments: request.attachments.map((attachment) => ({
          id: makeId("attachment"),
          filename: attachment.filename,
          media_type: attachment.media_type,
          size_bytes: Math.floor((attachment.data_base64.length * 3) / 4),
          url: `data:${attachment.media_type};base64,${attachment.data_base64}`,
        })),
      });
    }
    if (request.use_memory) {
      await wait(state.delay_ms, controller.signal);
      setProgress("memory-search", "Searched Memory", "running");
      await wait(state.delay_ms, controller.signal);
      setProgress("memory-search", "Searched Memory", "succeeded");
      setProgress("memory-read", "Read Memory", "running");
      memory_refs = clone(
        (state.library[request.guru_id] ?? []).slice(0, 2),
      ).map(({ id, kind, title, excerpt, as_of }, index) => ({
        record_id: id,
        kind,
        title,
        excerpt,
        as_of,
        section: index === 0 ? "Review sequence" : undefined,
        access:
          index === 0
            ? ("exact_read" as const)
            : ("search_discovered" as const),
      }));
      observer({
        type: "memory",
        run_id,
        memories: clone(memory_refs),
      });
      setProgress("memory-read", "Read Memory", "succeeded");
    } else {
      await wait(state.delay_ms, controller.signal);
    }

    const response = request.use_memory
      ? [
          "### Assessment",
          "",
          `I reviewed “${request.prompt}” with the Guru's existing Memory.`,
          "",
          "- **What we know:** The current evidence is not enough to declare lasting damage to business quality.",
          "- **How to frame it:** Separate the drivers into price, volume, one-time items, and reinvestment.",
          "- **Next check:** Revisit underlying margins and customer retention in the next filing.",
        ].join("\n")
      : [
          "### Assessment",
          "",
          `I reviewed “${request.prompt}” using only this conversation.`,
          "",
          "- **What we know:** Separate observed facts from assumptions before drawing a conclusion.",
          "- **Unresolved:** Leave unverified points open.",
          "- **Next check:** Turn on Guru Memory to compare this view with your existing criteria.",
        ].join("\n");

    const chunks = response.match(/[\s\S]{1,24}/gu) ?? [response];
    for (const text of chunks) {
      await wait(state.delay_ms, controller.signal);
      content += text;
      observer({ type: "delta", run_id, text });
    }

    let memory_update: MemoryUpdateResult | undefined;
    if (request.update_memory) {
      await wait(state.delay_ms, controller.signal);
      const commitId = makeId("memory-update");
      const recordId = "lens:quality/earnings-quality";
      const record = (state.library[request.guru_id] ?? []).find(
        (item) => item.id === recordId,
      );
      const beforeMarkdown = record?.markdown ?? "";
      const afterMarkdown = beforeMarkdown
        ? `${beforeMarkdown}\n\n# Latest review\n\nUpdated from this Chat.`
        : "---\nid: lens:quality/earnings-quality\ntitle: Earnings quality review\nsummary: Review recurring earnings quality.\nas_of: 2026-08-15T00:00:00Z\n---\n\n# Latest review\n\nUpdated from this Chat.";
      (state.memoryPrevious[request.guru_id] ??= {})[recordId] = beforeMarkdown;
      if (record) record.markdown = afterMarkdown;
      const update: MemoryUpdateResult = {
        status: "applied",
        commitId,
        changes: [
          {
            recordId,
            kind: "Lens",
            operation: "revise",
            title: "Earnings quality review",
            lesson: "Separate recurring earnings quality from one-time margin movement.",
            basis: "Current Chat evidence",
            futureUse: "This will change the checks used in later earnings research.",
          },
        ],
      };
      memory_update = update;
      observer({ type: "memory_update", run_id, result: clone(update) });
    }

    if (thread?.title === "New chat" && thread.messages.length === 1) {
      setProgress("title", "Named this chat", "running");
      await wait(state.delay_ms, controller.signal);
      const title = mockChatTitle(request.prompt);
      thread.title = title;
      observer({ type: "title", run_id, title });
      setProgress("title", "Named this chat", "succeeded");
    }

    const artifact = mockArtifactForPrompt(
      state,
      request,
      message_id,
      created_at,
    );
    if (artifact) {
      observer({ type: "artifact", run_id, artifact: clone(artifact) });
    }

    progress.finishedAtMs = Date.now();
    observer({
      type: "completed",
      run_id,
      message_id,
      final_text: content,
      created_at,
      execution_model: clone(execution_model),
      agent_harness: clone(agent_harness),
    });
    if (thread) {
      const assistant: ChatMessage = {
        id: message_id,
        role: "assistant",
        content,
        created_at,
        status: "complete",
        memory_refs: clone(memory_refs),
        memory_update,
        memory_revision: request.use_memory
          ? "mock-tree-revision"
          : undefined,
        execution_model: clone(execution_model),
        agent_harness,
        artifact_refs: artifact ? [artifact] : undefined,
        progress: clone(progress),
      };
      thread.messages.push(assistant);
      thread.updated_at = created_at;
    }
  } catch (error) {
    if (error instanceof DOMException && error.name === "AbortError") {
      progress.finishedAtMs = Date.now();
      for (const item of progress.items) {
        if (item.kind !== "commentary" && item.status === "running") {
          item.status = "stopped";
        }
      }
      if (thread) {
        thread.messages.push({
          id: message_id,
          role: "assistant",
          content: content || "Response stopped.",
          created_at,
          status: "aborted",
          memory_refs: clone(memory_refs),
          memory_revision: request.use_memory
            ? "mock-tree-revision"
            : undefined,
          execution_model: clone(execution_model),
          agent_harness,
          progress: clone(progress),
        });
        thread.updated_at = created_at;
      }
      observer({
        type: "aborted",
        run_id,
      });
    } else {
      progress.finishedAtMs = Date.now();
      for (const item of progress.items) {
        if (item.kind !== "commentary" && item.status === "running") {
          item.status = "failed";
          item.finishedAtMs = progress.finishedAtMs;
        }
      }
      if (thread) {
        thread.messages.push({
          id: message_id,
          role: "assistant",
          content: "Response could not be completed.",
          created_at,
          status: "error",
          memory_revision: request.use_memory
            ? "mock-tree-revision"
            : undefined,
          execution_model: clone(execution_model),
          agent_harness,
          progress: clone(progress),
        });
        thread.updated_at = created_at;
      }
      observer({
        type: "error",
        run_id,
        message: "Response could not be completed.",
        message_id,
        final_text: "Response could not be completed.",
        created_at,
        execution_model: clone(execution_model),
        agent_harness: clone(agent_harness),
        progress: clone(progress),
      });
    }
  } finally {
    state.chat_runs.delete(run_id);
    finishRunActivity(state, run_id);
  }
};

export const chatAbort = async (state: MockBridgeState, run_id: string) => {
  const run = state.chat_runs.get(run_id);
  if (!run) throw new Error("Active Chat run not found.");
  run.abort();
};
