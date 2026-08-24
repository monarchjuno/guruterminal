import { MockGuruTerminalBridge } from "../bridge";
import type { GuruWorkspace } from "../types";
import { GuruRunRegistry } from "./guruRunRegistry";

describe("GuruRunRegistry", () => {
  it("reports hydration failure and allows the same Guru to retry", async () => {
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    vi.spyOn(bridge, "runActivityList")
      .mockRejectedValueOnce(new Error("temporary native failure"))
      .mockResolvedValue([]);
    const registry = new GuruRunRegistry(bridge);

    await registry.hydrateGuru("guru-quality");
    expect(registry.getGuruSnapshot("guru-quality").hydration).toBe("error");
    await registry.hydrateGuru("guru-quality");
    expect(registry.getGuruSnapshot("guru-quality").hydration).toBe("ready");
    expect(bridge.runActivityList).toHaveBeenCalledTimes(2);
  });

  it("isolates recovered Chat runs by Guru and thread and retries exact Stop", async () => {
    vi.useFakeTimers();
    try {
      const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
      const activities = [
        {
          run_id: "chat-quality-margin",
          guru_id: "guru-quality",
          kind: "chat" as const,
          target: "thread-margin",
          started_at_ms: 1,
        },
        {
          run_id: "chat-quality-capital",
          guru_id: "guru-quality",
          kind: "chat" as const,
          target: "thread-capital",
          started_at_ms: 2,
        },
        {
          run_id: "chat-value-downside",
          guru_id: "guru-value",
          kind: "chat" as const,
          target: "thread-downside",
          started_at_ms: 3,
        },
      ];
      vi.spyOn(bridge, "runActivityList").mockResolvedValue(activities);
      const chatAbort = vi
        .spyOn(bridge, "chatAbort")
        .mockRejectedValueOnce(new Error("Active Chat run not found."))
        .mockResolvedValue();
      const registry = new GuruRunRegistry(bridge);

      await registry.hydrateActivities(["guru-quality", "guru-value"]);
      expect(
        registry.getRecoveredChat("guru-quality", "thread-margin"),
      ).toMatchObject({
        run_id: "chat-quality-margin",
        status: "running",
      });
      expect(
        registry.getRecoveredChat("guru-quality", "thread-capital"),
      ).toMatchObject({ run_id: "chat-quality-capital" });
      expect(
        registry.getRecoveredChat("guru-value", "thread-downside"),
      ).toMatchObject({ run_id: "chat-value-downside" });
      expect(registry.getSnapshot().active_guru_ids).toEqual(
        new Set(["guru-quality", "guru-value"]),
      );

      expect(registry.abortRecoveredChat("guru-quality", "thread-margin")).toBe(
        true,
      );
      await vi.advanceTimersByTimeAsync(100);
      expect(chatAbort.mock.calls).toEqual([
        ["chat-quality-margin"],
        ["chat-quality-margin"],
      ]);
      expect(
        registry.getRecoveredChat("guru-quality", "thread-capital")
          ?.abort_requested,
      ).toBe(false);
      expect(
        registry.getRecoveredChat("guru-value", "thread-downside")
          ?.abort_requested,
      ).toBe(false);
      registry.dispose();
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not attach a native Chat already owned by the live AI SDK registry", async () => {
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    vi.spyOn(bridge, "runActivityList").mockResolvedValue([
      {
        run_id: "chat-local-owned",
        guru_id: "guru-quality",
        kind: "chat",
        target: "thread-margin",
        started_at_ms: 1,
      },
    ]);
    const registry = new GuruRunRegistry(bridge, {
      isLocalChatActive: (guruId, threadId) =>
        guruId === "guru-quality" && threadId === "thread-margin",
    });

    await registry.hydrateActivities(["guru-quality"]);
    expect(
      registry.getRecoveredChat("guru-quality", "thread-margin"),
    ).toBeUndefined();
    expect(registry.getSnapshot().active_chat_threads).toEqual([]);
  });

  it("keeps a completed recovered Chat reconciling until canonical SQLite state is applied", async () => {
    vi.useFakeTimers();
    try {
      const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
      const canonical = await bridge.guruSelect("guru-quality");
      canonical.threads[0]?.messages.push({
        id: "assistant-recovered",
        role: "assistant",
        content: "Recovered canonical response",
        created_at: "2026-08-10T00:00:00.000Z",
        status: "complete",
      });
      vi.spyOn(bridge, "runActivityList")
        .mockResolvedValueOnce([
          {
            run_id: "chat-reconcile",
            guru_id: "guru-quality",
            kind: "chat",
            target: "thread-margin",
            started_at_ms: 1,
          },
        ])
        .mockResolvedValue([]);
      let resolveCanonical!: (workspace: GuruWorkspace) => void;
      vi.spyOn(bridge, "guruSelect").mockImplementation(
        () =>
          new Promise<GuruWorkspace>((resolve) => {
            resolveCanonical = resolve;
          }),
      );
      const reconciled = vi.fn();
      const registry = new GuruRunRegistry(bridge, {
        onRecoveredChatReconciled: reconciled,
      });

      await registry.hydrateActivities(["guru-quality"]);
      expect(
        registry.getRecoveredChat("guru-quality", "thread-margin")?.status,
      ).toBe("running");
      vi.advanceTimersByTime(0);
      await Promise.resolve();
      await Promise.resolve();
      expect(
        registry.getRecoveredChat("guru-quality", "thread-margin")?.status,
      ).toBe("reconciling");

      resolveCanonical(canonical);
      await Promise.resolve();
      await Promise.resolve();
      expect(reconciled).toHaveBeenCalledWith(canonical, "thread-margin");
      expect(
        registry.getRecoveredChat("guru-quality", "thread-margin"),
      ).toBeUndefined();
      registry.dispose();
    } finally {
      vi.useRealTimers();
    }
  });
});
