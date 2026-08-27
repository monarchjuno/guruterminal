import {
  applyChatProgressPatch,
  applyChatProgressSnapshot,
  fromGuruUIMessage,
  TauriChatTransport,
  toGuruUIMessage,
} from "./ai-sdk";
import type { GuruUIMessage } from "./ai-sdk";
import type {
  ChatMessage,
  ChatSendRequest,
  ChatStreamEvent,
  GuruTerminalBridge,
} from "../types";

describe("incremental chat progress", () => {
  const initial = {
    sequence: 0,
    progress: {
      startedAtMs: 10,
      items: [
        {
          id: "tool-1",
          kind: "tool" as const,
          category: "web" as const,
          operation: "search" as const,
          action: "Searched the web",
          status: "running" as const,
        },
      ],
    },
  };

  it("applies ordered upserts, removals, and the terminal finish", () => {
    const updated = applyChatProgressPatch(initial, {
      sequence: 1,
      upsertItems: [
        { ...initial.progress.items[0], status: "succeeded" },
        { id: "commentary-2", kind: "commentary", text: "Checked it." },
      ],
    });
    const finished = applyChatProgressPatch(updated, {
      sequence: 2,
      removeItemIds: ["commentary-2"],
      finishedAtMs: 20,
    });

    expect(finished).toEqual({
      sequence: 2,
      progress: {
        startedAtMs: 10,
        finishedAtMs: 20,
        items: [{ ...initial.progress.items[0], status: "succeeded" }],
      },
    });
  });

  it("ignores stale and gapped patches until a later full snapshot", () => {
    expect(
      applyChatProgressPatch(initial, {
        sequence: 2,
        finishedAtMs: 20,
      }),
    ).toBe(initial);
    expect(
      applyChatProgressPatch(initial, {
        sequence: 0,
        removeItemIds: ["tool-1"],
      }),
    ).toBe(initial);

    const replacement = {
      startedAtMs: 10,
      finishedAtMs: 30,
      items: [],
    };
    expect(applyChatProgressSnapshot(initial, replacement, 3)).toEqual({
      sequence: 3,
      progress: replacement,
    });
    expect(applyChatProgressSnapshot(initial, replacement, 0)).toBe(initial);
  });
});

describe("Guru chat UI message conversion", () => {
  it("keeps live progress commentary out of the answer position", () => {
    const message: GuruUIMessage = {
      id: "assistant-live",
      role: "assistant",
      metadata: { status: "streaming" },
      parts: [
        { type: "text", text: "", state: "streaming" },
        {
          type: "data-progress",
          id: "progress-live",
          data: {
            startedAtMs: 1,
            items: [
              {
                id: "commentary-1",
                kind: "commentary",
                text: "Draft answer text",
              },
            ],
          },
        },
      ],
    };

    const projected = fromGuruUIMessage(message);
    expect(projected.content).toBe("");
    expect(projected.progress?.items).toHaveLength(1);
    expect(projected.progress?.items[0]).toMatchObject({
      kind: "commentary",
      text: "Draft answer text",
    });
  });

  it("projects only the active assistant draft into the live answer position", () => {
    const live: GuruUIMessage = {
      id: "assistant-draft",
      role: "assistant",
      metadata: { status: "streaming" },
      parts: [
        { type: "reasoning", text: "Old tool preamble", state: "done" },
        { type: "reasoning", text: "Writing the answer", state: "streaming" },
      ],
    };

    expect(fromGuruUIMessage(live).content).toBe("Writing the answer");
    expect(
      fromGuruUIMessage({
        ...live,
        parts: live.parts.map((part) =>
          part.type === "reasoning" ? { ...part, state: "done" as const } : part,
        ),
      }).content,
    ).toBe("");
  });

  it("uses a stable timestamp fallback for malformed UI messages", () => {
    const message: GuruUIMessage = {
      id: "assistant-missing-created-at",
      role: "assistant",
      parts: [{ type: "text", text: "Recovered text", state: "done" }],
    };

    expect(fromGuruUIMessage(message).created_at).toBe(
      "1970-01-01T00:00:00.000Z",
    );
  });

  it("keeps progress when projecting a completed durable message", () => {
    const message: ChatMessage = {
      id: "assistant-1",
      role: "assistant",
      content: "Done",
      created_at: "2026-08-09T00:00:00.000Z",
      status: "complete",
      progress: {
        startedAtMs: 1,
        finishedAtMs: 2,
        items: [
          {
            id: "commentary-1",
            kind: "commentary",
            text: "Checking the source.",
          },
          {
            id: "tool-2",
            kind: "tool",
            category: "memory",
            operation: "read",
            action: "Read Memory",
            status: "succeeded",
          },
        ],
      },
    };

    const restored = fromGuruUIMessage(toGuruUIMessage(message));

    expect(restored.progress).toEqual(message.progress);
    expect(restored.content).toBe("Done");
  });

  it("uses a canonical error terminal instead of streamed partial text", () => {
    const projected = fromGuruUIMessage({
      id: "assistant-local",
      role: "assistant",
      metadata: {
        native_message_id: "assistant-canonical",
        created_at: "2026-08-09T00:00:01.000Z",
        status: "error",
        final_text: "Response could not be completed.",
      },
      parts: [
        {
          type: "text",
          text: "Partial provider response that must not survive.",
          state: "done",
        },
      ],
    });

    expect(projected).toMatchObject({
      id: "assistant-canonical",
      status: "error",
      content: "Response could not be completed.",
      created_at: "2026-08-09T00:00:01.000Z",
    });
  });

  it("restores immutable artifact references from AI SDK data parts", () => {
    const message: ChatMessage = {
      id: "assistant-artifact",
      role: "assistant",
      content: "The chart is ready.",
      created_at: "2026-08-09T00:00:00.000Z",
      artifact_refs: [
        {
          artifact_id: "artifact-1",
          revision: 2,
          kind: "chart",
          title: "Price history",
          digest: "a".repeat(64),
        },
      ],
    };

    expect(fromGuruUIMessage(toGuruUIMessage(message)).artifact_refs).toEqual(
      message.artifact_refs,
    );
  });

  it("restores persisted chat attachment metadata", () => {
    const message: ChatMessage = {
      id: "user-attachment",
      role: "user",
      content: "Review this",
      created_at: "2026-08-09T00:00:00.000Z",
      attachments: [
        {
          id: "attachment-1",
          filename: "chart.png",
          media_type: "image/png",
          size_bytes: 42,
        },
      ],
    };

    expect(fromGuruUIMessage(toGuruUIMessage(message)).attachments).toEqual(
      message.attachments,
    );
  });

  it("round-trips the exact Agent harness snapshot", () => {
    const message: ChatMessage = {
      id: "assistant-harness",
      role: "assistant",
      content: "Bound to this harness.",
      created_at: "2026-08-09T00:00:00.000Z",
      agent_harness: {
        schema: "guruterminal-harness/1",
        mode: "chat",
        skill_ids: ["research", "stress"],
        capability_ids: ["guruterminal.finance-core"],
        digest: "e".repeat(64),
      },
    };

    expect(fromGuruUIMessage(toGuruUIMessage(message)).agent_harness).toEqual(
      message.agent_harness,
    );
  });

  it("round-trips content-free latency metrics", () => {
    const message: ChatMessage = {
      id: "assistant-performance",
      role: "assistant",
      content: "Measured response",
      created_at: "2026-08-09T00:00:00.000Z",
      performance: {
        setupMs: 120,
        firstTextMs: 480,
        generationMs: 900,
        totalMs: 940,
        sessionCache: "warm",
        inputTokens: 320,
        outputTokens: 96,
        cacheReadTokens: 256,
        cacheWriteTokens: 64,
      },
    };

    expect(fromGuruUIMessage(toGuruUIMessage(message)).performance).toEqual(
      message.performance,
    );
  });

  it("round-trips a sealed Chat decision as structured data", () => {
    const message: ChatMessage = {
      id: "assistant-decision",
      role: "assistant",
      content: "My recommendation is below.",
      created_at: "2026-08-09T00:00:00.000Z",
      decision: {
        payload: {
          stance: "positive",
          horizon: "12 months",
          probability: 0.7,
          thesis: "The evidence supports a measured positive stance.",
          evidence_ids: ["evidence:company/filing"],
          risks: ["Demand weakens"],
          invalidation_conditions: [
            "Retention falls below the stated threshold",
          ],
        },
        digest: "d".repeat(64),
        sealed_at_ms: 1_754_707_200_000,
      },
    };

    expect(fromGuruUIMessage(toGuruUIMessage(message)).decision).toEqual(
      message.decision,
    );
  });
});

describe("Guru chat native stream transport", () => {
  it("coalesces adjacent draft deltas before they reach the AI SDK reducer", async () => {
    const bridge = {
      chatSend: vi.fn(
        async (
          request: ChatSendRequest,
          observer: (event: ChatStreamEvent) => void,
        ) => {
          observer({ type: "started", run_id: request.run_id });
          observer({
            type: "assistant_draft_started",
            run_id: request.run_id,
            draft_id: "draft-1",
          });
          observer({
            type: "assistant_draft_delta",
            run_id: request.run_id,
            draft_id: "draft-1",
            delta: "first ",
          });
          observer({
            type: "assistant_draft_delta",
            run_id: request.run_id,
            draft_id: "draft-1",
            delta: "second",
          });
          observer({
            type: "assistant_draft_finished",
            run_id: request.run_id,
            draft_id: "draft-1",
            disposition: "discarded",
          });
          observer({ type: "aborted", run_id: request.run_id });
          return { run_id: request.run_id };
        },
      ),
      chatAbort: vi.fn(async () => undefined),
    } as unknown as GuruTerminalBridge;
    const transport = new TauriChatTransport(bridge);
    const stream = await transport.sendMessages({
      trigger: "submit-message",
      chatId: "thread-1",
      messageId: undefined,
      messages: [
        {
          id: "user-1",
          role: "user",
          parts: [{ type: "text", text: "Hello", state: "done" }],
        },
      ],
      abortSignal: undefined,
      body: {
        guru_id: "guru-1",
        thread_id: "thread-1",
        use_memory: false,
        update_memory: false,
        model_profile_id: "model-1",
        thinking_level: "medium",
        run_options: {},
      },
    });
    const chunks = [];
    const reader = stream.getReader();
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(value);
    }

    expect(chunks.filter((chunk) => chunk.type === "reasoning-delta")).toEqual([
      {
        type: "reasoning-delta",
        id: "draft-1",
        delta: "first second",
      },
    ]);
  });
});
