import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../../App";
import { MockGuruTerminalBridge } from "../../bridge";
import { chooseGuru, openApp } from "../renderApp";

describe("Guru Terminal · Chat streaming and concurrency", () => {
  it("shows the submitted prompt and live response status before progress arrives", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    vi.spyOn(bridge, "chatSend").mockImplementation(
      async (request, observer) => {
        observer({ type: "started", run_id: request.run_id });
        return { run_id: request.run_id };
      },
    );
    await openApp(bridge);

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "오늘 하이닉스 어땠어?",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));

    expect(await screen.findByText("오늘 하이닉스 어땠어?")).toBeVisible();
    const progress = await screen.findByLabelText("Work progress");
    expect(
      within(progress).getByRole("button", { name: /Working · Preparing response ·/ }),
    ).toBeVisible();
    expect(screen.queryByText(/Provider reasoning/)).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /Reasoning/ }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Stop response" })).toBeVisible();
  });

  it("shows an inline work timeline only while the answer is live", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 150 });
    await openApp(bridge);

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Stream this answer",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));

    const process = await screen.findByLabelText("Work progress");
    await waitFor(() => {
      expect(
        within(process).getByRole("button", { name: /Working ·/ }),
      ).toHaveAttribute("aria-expanded", "true");
      expect(process).toHaveTextContent("Searched Memory");
    });

    const liveResponse = await screen.findByText("### Assessment", {
      exact: false,
    });
    const message = liveResponse.closest("article");
    expect(message).toHaveClass("streaming");
    expect(liveResponse).toHaveClass("message-response-live");
    expect(
      message!.querySelector('[data-streamdown="heading-3"]'),
    ).toBeNull();
    const liveMemoryGroup = await within(process).findByRole("button", {
      name: /Memory · 2 actions/,
    });
    if (liveMemoryGroup.getAttribute("aria-expanded") === "false") {
      await user.click(liveMemoryGroup);
    }
    expect(process).toHaveTextContent("Read Memory");

    await waitFor(() => expect(message).toHaveClass("complete"), {
      timeout: 15_000,
    });
    const heading = await screen.findByRole("heading", {
      name: "Assessment",
      level: 3,
    });
    expect(heading.closest("article")).toBe(message);
    expect(
      message!.querySelector('[data-streamdown="unordered-list"]'),
    ).toBeVisible();
    expect(
      within(message!).getByText("What we know:", { exact: true }),
    ).toBeVisible();
    expect(within(message!).getByLabelText("Work progress")).toBeInTheDocument();
    expect(
      within(message!).getByRole("button", { name: /steps ·/ }),
    ).toHaveAttribute("aria-expanded", "false");
  });

  it("replaces provisional tool and retry text with the authoritative final Chat text", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    let finishRun!: () => void;
    const runGate = new Promise<void>((resolve) => {
      finishRun = resolve;
    });
    vi.spyOn(bridge, "chatSend").mockImplementation(
      async (request, observer) => {
        observer({ type: "started", run_id: request.run_id });
        const progress = {
          startedAtMs: 1,
          items: [
            {
              id: "commentary-1",
              kind: "commentary" as const,
              text: "Tool preamble that must not become the final answer.",
            },
            {
              id: "system-2",
              kind: "system" as const,
              category: "system" as const,
              operation: "retry" as const,
              action: "Retrying model request",
              status: "succeeded" as const,
            },
          ],
        };
        observer({ type: "progress", run_id: request.run_id, progress });
        await runGate;
        observer({
          type: "completed",
          run_id: request.run_id,
          message_id: "message-authoritative-final",
          final_text: "Authoritative final answer.",
          created_at: new Date().toISOString(),
          execution_model: {
            profile_id: "model-test",
            name: "GPT-5.6 Luna",
            provider: "openai-codex",
            model: "gpt-5.6-luna",
            thinking_level: "medium",
            run_options: {},
          },
          agent_harness: {
            schema: "guruterminal-agent-harness/1",
            mode: "chat",
            skill_ids: [],
            capability_ids: [],
            digest: "b".repeat(64),
          },
        });
        return { run_id: request.run_id };
      },
    );

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Return only the settled final answer",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));

    const liveCommentary = await screen.findByText(
      "Tool preamble that must not become the final answer.",
    );
    const assistantMessage = liveCommentary.closest("article");
    expect(assistantMessage?.querySelector(".message-content")).toBeNull();
    expect(
      screen.queryByText("Authoritative final answer."),
    ).not.toBeInTheDocument();

    finishRun();
    const finalAnswer = await screen.findByText("Authoritative final answer.");
    expect(finalAnswer).toBeVisible();
    const settledMessage = finalAnswer.closest("article");
    expect(settledMessage).toHaveTextContent("Authoritative final answer.");
    expect(
      within(settledMessage!).getByLabelText("Work progress"),
    ).toBeInTheDocument();
  });

  it("settles repeated partial deltas at a canonical terminal error", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.spyOn(bridge, "chatSend").mockImplementation(async (request, observer) => {
      observer({ type: "started", run_id: request.run_id });
      for (let line = 1; line <= 10; line += 1) {
        observer({
          type: "delta",
          run_id: request.run_id,
          text: `Partial line ${line}.\n`,
        });
      }
      observer({
        type: "error",
        run_id: request.run_id,
        message: "Response could not be completed.",
        message_id: "assistant-canonical-stream-error",
        final_text: "Response could not be completed.",
        created_at: "2026-08-25T00:00:01.000Z",
        execution_model: {
          profile_id: "model-test",
          name: "GPT-5.6 Luna",
          provider: "openai-codex",
          model: "gpt-5.6-luna",
          thinking_level: "max",
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
    try {
      await openApp(bridge);
      await user.type(
        screen.getByRole("textbox", { name: "Message Guru" }),
        "Return a terminal error after streaming.",
      );
      await user.click(screen.getByRole("button", { name: "Send" }));

      const terminal = await screen.findByText(
        (content, element) =>
          content === "Response could not be completed." &&
          Boolean(element?.closest("article")),
      );
      expect(terminal.closest("article")).toHaveClass("error");
      expect(screen.queryByText("Partial line 10.")).not.toBeInTheDocument();
      await waitFor(() =>
        expect(
          screen.getByRole("textbox", { name: "Message Guru" }),
        ).toBeEnabled(),
      );
      expect(
        consoleError.mock.calls
          .flat()
          .map((value) => String(value))
          .join(" "),
      ).not.toContain("Maximum update depth exceeded");
    } finally {
      consoleError.mockRestore();
    }
  });

  it("runs distinct Gurus concurrently and stops only the selected thread", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 100 });
    const chatSend = vi.spyOn(bridge, "chatSend");
    const chatAbort = vi.spyOn(bridge, "chatAbort");
    await openApp(bridge);

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Keep the quality run active",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByRole("status", {
      name: "Quality Compounder has active sessions",
    });

    await chooseGuru(user, "Contrarian Value");
    await screen.findByRole("heading", { name: "Downside scenario review" });
    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Keep the value run active too",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() => expect(chatSend).toHaveBeenCalledTimes(2));
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
        name: "Downside scenario review is running",
      }),
    ).toBeVisible();

    const firstRun = await chatSend.mock.results[0]!.value;
    const secondRun = await chatSend.mock.results[1]!.value;
    await chooseGuru(user, "Quality Compounder");
    await screen.findByRole("heading", {
      name: "How should we read the margin decline?",
    });
    await user.click(
      await screen.findByRole("button", { name: "Stop response" }),
    );
    await waitFor(() =>
      expect(chatAbort).toHaveBeenCalledWith(firstRun.run_id),
    );
    expect(chatAbort).not.toHaveBeenCalledWith(secondRun.run_id);

    await chooseGuru(user, "Contrarian Value");
    await screen.findByRole("heading", { name: "Downside scenario review" });
    expect(screen.getByRole("button", { name: "Stop response" })).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Stop response" }));
    await waitFor(() =>
      expect(chatAbort).toHaveBeenCalledWith(secondRun.run_id),
    );
  });

  it("waits for the canonical aborted message before settling Stop", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 100 });
    await openApp(bridge);

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Persist this stopped turn",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));
    await user.click(
      await screen.findByRole("button", { name: "Stop response" }),
    );

    await waitFor(() =>
      expect(
        document.querySelector("article.message.assistant.aborted"),
      ).toBeVisible(),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("textbox", { name: "Message Guru" }),
      ).toBeEnabled(),
    );
    const workspace = await bridge.guruSelect("guru-quality");
    expect(workspace.threads[0]?.messages.at(-1)?.status).toBe("aborted");
  });

  it.each([
    {
      status: "complete" as const,
      content: "Canonical completion won the Stop race.",
    },
    {
      status: "error" as const,
      content: "Canonical failure won the Stop race.",
    },
  ])(
    "keeps the canonical $status terminal when Stop loses the terminal race",
    async ({ status, content }) => {
      const user = userEvent.setup();
      const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
      const originalSelect = bridge.guruSelect.bind(bridge);
      const stale = await originalSelect("guru-quality");
      const canonical = structuredClone(stale);
      const thread = canonical.threads[0]!;
      thread.messages.push(
        {
          id: "user-stop-terminal-race",
          role: "user",
          content: "Race the native terminal boundary",
          created_at: "2026-08-25T00:00:00.000Z",
          status: "complete",
        },
        {
          id: `assistant-stop-terminal-${status}`,
          role: "assistant",
          content,
          created_at: "2026-08-25T00:00:01.000Z",
          status,
        },
      );
      let selectionCount = 0;
      vi.spyOn(bridge, "guruSelect").mockImplementation((guruId) => {
        if (guruId !== "guru-quality") return originalSelect(guruId);
        selectionCount += 1;
        return Promise.resolve(selectionCount === 1 ? stale : canonical);
      });
      vi.spyOn(bridge, "runActivityList").mockResolvedValue([]);
      const chatAbort = vi.spyOn(bridge, "chatAbort").mockResolvedValue();
      vi.spyOn(bridge, "chatSend").mockImplementation(async (request, observer) => {
        observer({ type: "started", run_id: request.run_id });
        observer({
          type: "delta",
          run_id: request.run_id,
          text: "Provisional response that must be replaced.",
        });
        return { run_id: request.run_id };
      });
      await openApp(bridge);

      await user.type(
        screen.getByRole("textbox", { name: "Message Guru" }),
        "Race the native terminal boundary",
      );
      await user.click(screen.getByRole("button", { name: "Send" }));
      await screen.findByText("Provisional response that must be replaced.");
      await user.click(screen.getByRole("button", { name: "Stop response" }));

      await waitFor(() => expect(chatAbort).toHaveBeenCalledTimes(1));
      expect(await screen.findByText(content)).toBeVisible();
      expect(
        screen.queryByText("Provisional response that must be replaced."),
      ).not.toBeInTheDocument();
      expect(
        document.querySelector(`article.message.assistant.${status}`),
      ).toBeVisible();
    },
  );

  it("keeps two threads of the same Guru independently live", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 100 });
    const chatSend = vi.spyOn(bridge, "chatSend");
    const chatAbort = vi.spyOn(bridge, "chatAbort");
    await openApp(bridge);

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "First quality thread",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByRole("status", {
      name: "How should we read the margin decline? is running",
    });
    expect(
      screen
        .getByRole("button", {
          name: "How should we read the margin decline?",
        })
        .querySelector("[data-slot=spinner]"),
    ).toBeInstanceOf(SVGElement);
    expect(
      screen
        .getByRole("button", { name: "Capital allocation checklist" })
        .querySelector("[data-slot=spinner]"),
    ).toBeNull();

    await user.click(
      screen.getByRole("button", { name: /Capital allocation checklist/ }),
    );
    await screen.findByRole("heading", {
      name: "Capital allocation checklist",
    });
    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Second quality thread",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(chatSend).toHaveBeenCalledTimes(2));
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
    expect(
      screen
        .getByRole("button", {
          name: "How should we read the margin decline?",
        })
        .querySelector("[data-slot=spinner]"),
    ).toBeInstanceOf(SVGElement);
    expect(
      screen
        .getByRole("button", { name: "Capital allocation checklist" })
        .querySelector("[data-slot=spinner]"),
    ).toBeInstanceOf(SVGElement);

    const firstRun = await chatSend.mock.results[0]!.value;
    const secondRun = await chatSend.mock.results[1]!.value;
    await user.click(
      screen.getByRole("button", {
        name: /How should we read the margin decline/,
      }),
    );
    await user.click(
      await screen.findByRole("button", { name: "Stop response" }),
    );
    await waitFor(() =>
      expect(chatAbort).toHaveBeenCalledWith(firstRun.run_id),
    );
    expect(chatAbort).not.toHaveBeenCalledWith(secondRun.run_id);

    await user.click(
      screen.getByRole("button", { name: /Capital allocation checklist/ }),
    );
    expect(screen.getByRole("button", { name: "Stop response" })).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Stop response" }));
  });

  it("keeps background titles and artifacts scoped to their exact Guru thread", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 35 });
    await openApp(bridge);

    await user.click(
      screen.getByRole("button", {
        name: "New session for Quality Compounder",
      }),
    );
    await screen.findByRole("heading", { name: "New chat" });
    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Background document",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByRole("status", {
      name: "Quality Compounder has active sessions",
    });

    await chooseGuru(user, "Contrarian Value");
    await screen.findByRole("heading", { name: "Downside scenario review" });
    await waitFor(
      () =>
        expect(
          screen.queryByRole("status", {
            name: "Quality Compounder has active sessions",
          }),
        ).not.toBeInTheDocument(),
      { timeout: 15_000 },
    );
    expect(
      screen.getByRole("heading", { name: "Downside scenario review" }),
    ).toBeVisible();
    expect(
      screen.queryByLabelText("Chat workspace panel"),
    ).not.toBeInTheDocument();

    await chooseGuru(user, "Quality Compounder");
    await screen.findByRole("heading", { name: "Background document" });
    expect(
      screen.queryByLabelText("Chat workspace panel"),
    ).not.toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: /Open document Research note/ }),
    );
    expect(await screen.findByLabelText("Chat workspace panel")).toBeVisible();
  });

  it("merges a late canonical Guru snapshot without losing a completed local stream", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 35 });
    const originalSelect = bridge.guruSelect.bind(bridge);
    await openApp(bridge);

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Preserve this response across a stale selection",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByLabelText("Work progress");
    const staleWorkspace = await originalSelect("guru-quality");
    staleWorkspace.threads[0] = {
      ...staleWorkspace.threads[0]!,
      title: "Canonical remote thread title",
      use_memory: false,
    };

    await chooseGuru(user, "Contrarian Value");
    await screen.findByRole("heading", { name: "Downside scenario review" });
    let resolveSelection!: (workspace: typeof staleWorkspace) => void;
    vi.spyOn(bridge, "guruSelect").mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveSelection = resolve;
        }),
    );
    await chooseGuru(user, "Quality Compounder");

    await waitFor(
      async () => {
        const latest = await originalSelect("guru-quality");
        expect(
          latest.threads[0]?.messages.some((message) =>
            message.content.includes(
              "Preserve this response across a stale selection",
            ),
          ),
        ).toBe(true);
      },
      { timeout: 15_000 },
    );
    await act(async () => resolveSelection(staleWorkspace));

    await screen.findByRole("heading", {
      name: "Canonical remote thread title",
    });
    expect(
      await screen.findByText(
        /I reviewed.*Preserve this response across a stale selection/,
      ),
    ).toBeVisible();
    expect(
      await screen.findByRole("checkbox", { name: "Use memory" }),
    ).not.toBeChecked();
  });

  it("rehydrates completed Chat messages from the native store after remount", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const firstMount = render(<App bridge={bridge} />);
    await screen.findByRole("heading", {
      name: "How should we read the margin decline?",
    });

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Persist this answer through an app remount",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByText("Response complete.");
    firstMount.unmount();

    render(<App bridge={bridge} />);
    await screen.findByRole("heading", {
      name: "How should we read the margin decline?",
    });
    expect(
      await screen.findByText(
        /I reviewed.*Persist this answer through an app remount/,
      ),
    ).toBeVisible();
    expect(screen.getByLabelText("Work progress")).toBeInTheDocument();
  });
});
