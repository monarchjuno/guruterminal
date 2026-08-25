import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MockGuruTerminalBridge } from "../../bridge";
import { openApp } from "../renderApp";

describe("Guru Terminal · Chat progress", () => {
  it("opens a projected web source in the chat workspace panel", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const openExternalUrl = vi.spyOn(bridge, "openExternalUrl");
    const browserTabOpen = vi.spyOn(bridge, "browserTabOpen");
    let finishRun!: () => void;
    const runGate = new Promise<void>((resolve) => {
      finishRun = resolve;
    });
    vi.spyOn(bridge, "chatSend").mockImplementation(
      async (request, observer) => {
        const progress = {
          startedAtMs: Date.now(),
          items: [
            {
              id: "web-1",
              kind: "tool" as const,
              category: "web" as const,
              operation: "read" as const,
              action: "Read a web source",
              target: "Rate outlook · example.com",
              href: "https://example.com/rates",
              status: "succeeded" as const,
            },
          ],
        };
        observer({ type: "started", run_id: request.run_id });
        observer({ type: "progress", run_id: request.run_id, progress });
        await runGate;
        observer({
          type: "completed",
          run_id: request.run_id,
          message_id: "assistant-web-source",
          final_text: "Source checked.",
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
    await openApp(bridge);

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Check this web source",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));

    const timeline = await screen.findByLabelText("Work progress");
    expect(timeline).not.toHaveTextContent("web:f6495eccf2c0cca04e4c6a03");
    await user.click(
      await within(timeline).findByRole("button", {
        name: "Rate outlook · example.com",
      }),
    );
    const workspace = await screen.findByLabelText("Chat workspace panel");
    expect(workspace).toBeVisible();
    expect(
      within(workspace).getByRole("tab", { name: "example.com" }),
    ).toBeVisible();
    expect(browserTabOpen).toHaveBeenCalledWith(
      expect.objectContaining({ url: "https://example.com/rates" }),
      expect.any(Function),
    );
    expect(openExternalUrl).not.toHaveBeenCalled();
    finishRun();
    await screen.findByText("Source checked.");
    const settled = await screen.findByLabelText("Work progress");
    expect(settled).toBeInTheDocument();
    expect(
      within(settled).getByRole("button", { name: /1 step · Web research/ }),
    ).toHaveAttribute("aria-expanded", "false");
    expect(await screen.findByLabelText("Chat workspace panel")).toBeVisible();
  });
});
