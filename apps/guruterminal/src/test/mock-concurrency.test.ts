import { waitFor } from "@testing-library/react";
import { MockGuruTerminalBridge } from "../bridge";
import type { ChatSendRequest } from "../types";
import {
  createMockBridgeState,
  finishRunActivity,
  registerRunActivity,
} from "../bridge/mock/state";

const chatRequest = (guru_id: string, thread_id: string): ChatSendRequest => ({
  run_id: `chat-${guru_id}-${crypto.randomUUID()}`,
  guru_id,
  thread_id,
  prompt: "Hold this mock worker slot",
  use_memory: true,
  update_memory: false,
  model_profile_id: "model-test",
  thinking_level: "max",
  run_options: {},
  attachments: [],
});

describe("Mock model-run admission", () => {
  it("matches the four-worker cap and exact-target conflict contract", async () => {
    const bridge = new MockGuruTerminalBridge({ delay_ms: 1_000 });
    const observer = () => undefined;

    const quality = await bridge.guruSelect("guru-quality");
    const value = await bridge.guruSelect("guru-value");
    const cycle = await bridge.guruSelect("guru-cycle");
    const extra = await bridge.chatCreate({ guru_id: "guru-quality" });
    const firstChat = await bridge.chatSend(
      chatRequest("guru-quality", quality.threads[0]!.id),
      observer,
    );
    await expect(
      bridge.chatSend(
        chatRequest("guru-quality", quality.threads[0]!.id),
        observer,
      ),
    ).rejects.toThrow(/exact Guru run target/i);
    const secondChat = await bridge.chatSend(
      chatRequest("guru-value", value.threads[0]!.id),
      observer,
    );
    const thirdChat = await bridge.chatSend(
      chatRequest("guru-cycle", cycle.threads[0]!.id),
      observer,
    );
    const fourthChat = await bridge.chatSend(
      chatRequest("guru-quality", extra.id),
      observer,
    );
    expect(await bridge.runActivityList()).toHaveLength(4);

    const overflow = await bridge.chatCreate({ guru_id: "guru-value" });
    await expect(
      bridge.chatSend(
        chatRequest("guru-value", overflow.id),
        observer,
      ),
    ).rejects.toThrow(/four model worker slots/i);

    await Promise.all([
      bridge.chatAbort(firstChat.run_id),
      bridge.chatAbort(secondChat.run_id),
      bridge.chatAbort(thirdChat.run_id),
      bridge.chatAbort(fourthChat.run_id),
    ]);
    await waitFor(async () =>
      expect(await bridge.runActivityList()).toHaveLength(0),
    );
  });

  it("rejects duplicate IDs, models Chat mutations as the same target, and makes a Memory write Guru-exclusive", () => {
    const state = createMockBridgeState(0);
    registerRunActivity(state, {
      run_id: "chat-a",
      guru_id: "guru-quality",
      kind: "chat",
      target: "thread-margin",
      started_at_ms: 1,
    });
    expect(() =>
      registerRunActivity(state, {
        run_id: "chat-a",
        guru_id: "guru-value",
        kind: "chat",
        target: "thread-value",
        started_at_ms: 2,
      }),
    ).toThrow(/same ID/i);
    expect(() =>
      registerRunActivity(state, {
        run_id: "rename-a",
        guru_id: "guru-quality",
        kind: "chat_mutation",
        target: "thread-margin",
        started_at_ms: 2,
      }),
    ).toThrow(/exact Guru run target/i);
    expect(() =>
      registerRunActivity(state, {
        run_id: "memory-write-a",
        guru_id: "guru-quality",
        kind: "memory_write",
        target: "memory-change-a",
        started_at_ms: 2,
      }),
    ).toThrow(/Memory reader or writer/i);

    finishRunActivity(state, "chat-a");
    registerRunActivity(state, {
      run_id: "memory-write-a",
      guru_id: "guru-quality",
      kind: "memory_write",
      target: "memory-change-a",
      started_at_ms: 3,
    });
    expect(() =>
      registerRunActivity(state, {
        run_id: "memory-change-a",
        guru_id: "guru-quality",
        kind: "memory_mutation",
        target: "memory-change-a",
        started_at_ms: 4,
      }),
    ).toThrow(/Memory reader or writer|exact Guru run target/i);

    registerRunActivity(state, {
      run_id: "chat-b",
      guru_id: "guru-value",
      kind: "chat",
      target: "thread-b",
      started_at_ms: 5,
    });
    expect(() =>
      registerRunActivity(state, {
        run_id: "chat-rename-b",
        guru_id: "guru-value",
        kind: "chat_mutation",
        target: "thread-b",
        started_at_ms: 6,
      }),
    ).toThrow(/exact Guru run target/i);
  });

  it("rejects active Guru deletion and stale exact abort IDs", async () => {
    const bridge = new MockGuruTerminalBridge({ delay_ms: 1_000 });
    const quality = await bridge.guruSelect("guru-quality");
    const chat = await bridge.chatSend(
      chatRequest("guru-quality", quality.threads[0]!.id),
      () => undefined,
    );
    await expect(
      bridge.guruDelete({ guru_id: "guru-quality" }),
    ).rejects.toThrow(/active sessions/i);
    await bridge.chatAbort(chat.run_id);
    await waitFor(async () =>
      expect(await bridge.runActivityList()).toHaveLength(0),
    );
    await expect(bridge.chatAbort(chat.run_id)).rejects.toThrow(
      /active Chat run not found/i,
    );
  });
});
