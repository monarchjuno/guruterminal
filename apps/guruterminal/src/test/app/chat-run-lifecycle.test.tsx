import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { App } from "../../App";
import { MockGuruTerminalBridge } from "../../bridge";
import type { GuruWorkspace } from "../../types";
import { openApp } from "../renderApp";

describe("Guru Terminal · Chat run lifecycle", () => {
  it("keeps the App-lifetime Chat registry active through the StrictMode effect probe", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const chatSend = vi.spyOn(bridge, "chatSend");
    render(
      <StrictMode>
        <App bridge={bridge} />
      </StrictMode>,
    );
    await screen.findByRole("heading", {
      name: "How should we read the margin decline?",
    });

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Strict lifecycle check",
    );
    await user.click(await screen.findByRole("button", { name: "Send" }));

    await waitFor(() => expect(chatSend).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(screen.getByText(/I reviewed “Strict lifecycle check”/)).toBeVisible(),
    );
  });

  it("recovers multiple native Chat targets in the sidebar and Stops only the visible exact run", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const activities = [
      {
        run_id: "chat-reload-margin",
        guru_id: "guru-quality",
        kind: "chat" as const,
        target: "thread-margin",
        started_at_ms: 1,
      },
      {
        run_id: "chat-reload-capital",
        guru_id: "guru-quality",
        kind: "chat" as const,
        target: "thread-capital",
        started_at_ms: 2,
      },
      {
        run_id: "chat-reload-downside",
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

    render(<App bridge={bridge} />);
    await screen.findByRole("heading", {
      name: "How should we read the margin decline?",
    });
    expect(
      screen.getByRole("status", {
        name: "Quality Compounder has active sessions",
      }),
    ).toBeVisible();
    expect(
      screen.getByRole("status", {
        name: "Contrarian Value has active sessions",
      }),
    ).toBeVisible();
    expect(
      screen.getByRole("status", {
        name: "How should we read the margin decline? is running",
      }),
    ).toBeVisible();
    expect(
      screen.getByRole("status", {
        name: "Capital allocation checklist is running",
      }),
    ).toBeVisible();
    expect(screen.getByText("Still working")).toBeVisible();
    expect(screen.getByRole("textbox", { name: "Message Guru" })).toBeEnabled();

    await user.click(screen.getByRole("button", { name: "Stop response" }));
    await waitFor(() => expect(chatAbort).toHaveBeenCalledTimes(2));
    expect(chatAbort.mock.calls).toEqual([
      ["chat-reload-margin"],
      ["chat-reload-margin"],
    ]);
  });

  it("hydrates active Chat before initial selection and rereads canonical completion", async () => {
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const originalSelect = bridge.guruSelect.bind(bridge);
    const stale = await originalSelect("guru-quality");
    const canonical = structuredClone(stale);
    canonical.threads[0]?.messages.push({
      id: "assistant-after-reload",
      role: "assistant",
      content: "Canonical response committed after reload.",
      created_at: "2026-08-10T00:00:00.000Z",
      status: "complete",
    });
    const runActivityList = vi
      .spyOn(bridge, "runActivityList")
      .mockResolvedValueOnce([
        {
          run_id: "chat-bootstrap-race",
          guru_id: "guru-quality",
          kind: "chat",
          target: "thread-margin",
          started_at_ms: 1,
        },
      ])
      .mockResolvedValue([]);
    let selectionCount = 0;
    let resolveRecovery!: (workspace: GuruWorkspace) => void;
    const guruSelect = vi
      .spyOn(bridge, "guruSelect")
      .mockImplementation((guruId) => {
        selectionCount += 1;
        if (guruId !== "guru-quality") return originalSelect(guruId);
        if (selectionCount === 1) return Promise.resolve(stale);
        return new Promise<GuruWorkspace>((resolve) => {
          resolveRecovery = resolve;
        });
      });

    render(<App bridge={bridge} />);
    await screen.findByRole("heading", {
      name: "How should we read the margin decline?",
    });
    await screen.findByText("Updating the answer");
    expect(runActivityList.mock.invocationCallOrder[0]).toBeLessThan(
      guruSelect.mock.invocationCallOrder[0]!,
    );
    expect(screen.getByRole("button", { name: "Stop response" })).toBeVisible();
    expect(screen.getByRole("textbox", { name: "Message Guru" })).toBeEnabled();

    await act(async () => resolveRecovery(canonical));
    expect(
      await screen.findByText("Canonical response committed after reload."),
    ).toBeVisible();
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Send" })).toBeVisible(),
    );
    expect(screen.getByRole("textbox", { name: "Message Guru" })).toBeEnabled();
  });

  it("stops the sealed Chat run before the native Started event arrives", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    let sealedRunId = "";
    vi.spyOn(bridge, "chatSend").mockImplementation(
      (request) =>
        new Promise(() => {
          sealedRunId = request.run_id;
        }),
    );
    const chatAbort = vi
      .spyOn(bridge, "chatAbort")
      .mockRejectedValueOnce(new Error("Active Chat run not found."))
      .mockResolvedValue();

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Stop during native preflight",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(sealedRunId).toMatch(/^chat-ui-/));
    await user.click(
      await screen.findByRole("button", { name: "Stop response" }),
    );

    await waitFor(() => expect(chatAbort).toHaveBeenCalledTimes(2));
    expect(chatAbort).toHaveBeenNthCalledWith(1, sealedRunId);
    expect(chatAbort).toHaveBeenNthCalledWith(2, sealedRunId);
  });
});
