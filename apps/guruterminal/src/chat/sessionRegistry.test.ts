import { MockGuruTerminalBridge } from "../bridge";
import type { ChatThread } from "../types";
import { fromGuruUIMessage } from "./ai-sdk";
import { ChatSessionRegistry } from "./sessionRegistry";

const thread = (id: string): ChatThread => ({
  id,
  guru_id: "guru-quality",
  title: id,
  use_memory: false,
  update_memory: false,
  messages: [],
  updated_at: "2026-08-12T00:00:00.000Z",
});

describe("ChatSessionRegistry durable progress", () => {
  it("keeps completed progress out of the answer text", async () => {
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    vi.spyOn(bridge, "chatSend").mockImplementation(
      async (request, observer) => {
        const label = request.thread_id;
        observer({ type: "started", run_id: request.run_id });
        observer({
          type: "progress",
          run_id: request.run_id,
          progress: {
            startedAtMs: 1,
            items: [
              {
                id: `commentary-${label}`,
                kind: "commentary",
                text: `${label} live commentary`,
              },
            ],
          },
        });
        observer({
          type: "completed",
          run_id: request.run_id,
          message_id: `assistant-${label}`,
          final_text: `Answer for ${label}`,
          created_at: "2026-08-12T00:00:01.000Z",
          execution_model: {
            profile_id: "model-test",
            name: "Test model",
            provider: "test",
            model: "test-model",
            thinking_level: "medium",
            run_options: {},
          },
          agent_harness: {
            schema: "guruterminal-harness/1",
            mode: "chat",
            skill_ids: [],
            capability_ids: [],
            digest: "a".repeat(64),
          },
        });
        return { run_id: request.run_id };
      },
    );

    const messages = vi.fn();
    const registry = new ChatSessionRegistry(bridge, {
      onArtifact: vi.fn(),
      onMessages: messages,
      onStatus: vi.fn(),
      onTitle: vi.fn(),
    });
    const first = registry.ensure(thread("thread-one"));
    const second = registry.ensure(thread("thread-two"));
    const options = (threadId: string) => ({
      body: {
        guru_id: "guru-quality",
        thread_id: threadId,
        use_memory: false,
        update_memory: false,
        model_profile_id: "model-test",
        thinking_level: "medium",
        run_options: {},
      },
    });

    await Promise.all([
      first.sendMessage({ text: "first" }, options("thread-one")),
      second.sendMessage({ text: "second" }, options("thread-two")),
    ]);

    expect(JSON.stringify(messages.mock.calls)).toContain(
      "Answer for thread-one",
    );
    expect(JSON.stringify(messages.mock.calls)).toContain(
      "Answer for thread-two",
    );
    expect(JSON.stringify(first.messages)).not.toContain("data-thinking");
    expect(JSON.stringify(second.messages)).not.toContain("data-thinking");
    expect(
      first.messages
        .map(fromGuruUIMessage)
        .find((message) => message.role === "assistant")?.content,
    ).toBe("Answer for thread-one");

    registry.dispose();
  });

  it("replaces streamed partial text with the canonical native error terminal", async () => {
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    vi.spyOn(bridge, "chatSend").mockImplementation(async (request, observer) => {
      observer({ type: "started", run_id: request.run_id });
      observer({
        type: "delta",
        run_id: request.run_id,
        text: "Partial provider answer.",
      });
      observer({
        type: "error",
        run_id: request.run_id,
        message: "Response could not be completed.",
        message_id: "assistant-canonical-error",
        final_text: "Response could not be completed.",
        created_at: "2026-08-12T00:00:01.000Z",
        execution_model: {
          profile_id: "model-test",
          name: "Test model",
          provider: "test",
          model: "test-model",
          thinking_level: "medium",
          run_options: {},
        },
        agent_harness: {
          schema: "guruterminal-harness/1",
          mode: "chat",
          skill_ids: [],
          capability_ids: [],
          digest: "a".repeat(64),
        },
      });
      return { run_id: request.run_id };
    });

    const registry = new ChatSessionRegistry(bridge, {
      onArtifact: vi.fn(),
      onMessages: vi.fn(),
      onStatus: vi.fn(),
      onTitle: vi.fn(),
    });
    const chat = registry.ensure(thread("thread-error"));
    try {
      await chat.sendMessage(
        { text: "cause a terminal error" },
        {
          body: {
            guru_id: "guru-quality",
            thread_id: "thread-error",
            use_memory: false,
            update_memory: false,
            model_profile_id: "model-test",
            thinking_level: "medium",
            run_options: {},
          },
        },
      );
    } catch {
      // AI SDK reports the terminal failure to the caller after it has applied
      // the native terminal metadata to the in-memory transcript.
    }
    await new Promise((resolve) => setTimeout(resolve, 0));

    const assistant = chat.messages
      .map(fromGuruUIMessage)
      .find((message) => message.role === "assistant");
    expect(assistant).toMatchObject({
      id: "assistant-canonical-error",
      status: "error",
      content: "Response could not be completed.",
      created_at: "2026-08-12T00:00:01.000Z",
    });

    registry.dispose();
  });
});
