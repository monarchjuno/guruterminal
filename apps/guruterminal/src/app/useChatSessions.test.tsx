import { act, renderHook } from "@testing-library/react";
import { MockGuruTerminalBridge } from "../bridge";
import type { ChatThread } from "../types";
import { useChatSessions } from "./useChatSessions";

const thread: ChatThread = {
  id: "thread-noop",
  guru_id: "guru-noop",
  title: "No-op state update",
  updated_at: "2026-08-25T00:00:00.000Z",
  use_memory: true,
  update_memory: false,
  messages: [],
};

describe("useChatSessions", () => {
  it("does not publish a new state object when a thread updater returns current", () => {
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const { result } = renderHook(() => useChatSessions(bridge));

    act(() => result.current.setThreadsForGuru(thread.guru_id, [thread]));
    const currentState = result.current.threadsByGuru;
    const currentRef = result.current.threadsByGuruRef.current;

    act(() =>
      result.current.setThreadsForGuru(thread.guru_id, (current) => current),
    );

    expect(result.current.threadsByGuru).toBe(currentState);
    expect(result.current.threadsByGuruRef.current).toBe(currentRef);
  });
});
