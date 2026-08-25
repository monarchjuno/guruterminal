import { fromGuruUIMessage, toGuruUIMessage } from "./ai-sdk";
import type { GuruUIMessage } from "./ai-sdk";
import type { ChatMessage } from "../types";

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
