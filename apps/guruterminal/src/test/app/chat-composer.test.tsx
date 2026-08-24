import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MockGuruTerminalBridge } from "../../bridge";
import { mainTab, openApp } from "../renderApp";

const modelMenuButton = () =>
  screen.getByRole("button", { name: "Model settings for this message" });

const chooseCatalogOption = async (
  user: ReturnType<typeof userEvent.setup>,
  name: string | RegExp,
) => {
  await user.click(await screen.findByRole("menuitemradio", { name }));
  await user.keyboard("{Escape}");
};

describe("Guru Terminal · Chat composer", () => {
  it("keeps an unsent Chat draft while moving between workspaces", async () => {
    const user = userEvent.setup();
    await openApp();
    const composer = screen.getByRole("textbox", { name: "Message Guru" });
    await user.type(composer, "Keep this draft while I check memory");

    await user.click(mainTab(/Memory/));
    await screen.findByRole("heading", { name: "Memory", level: 1 });
    await user.click(mainTab(/Chat/));

    expect(screen.getByRole("textbox", { name: "Message Guru" })).toHaveValue(
      "Keep this draft while I check memory",
    );
  });

  it("does not submit while an IME composition is active", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const chatSend = vi.spyOn(bridge, "chatSend");
    const composer = screen.getByRole("textbox", { name: "Message Guru" });
    await user.type(composer, "Composed input");

    fireEvent.keyDown(composer, {
      key: "Enter",
      code: "Enter",
      isComposing: true,
      keyCode: 229,
    });

    expect(chatSend).not.toHaveBeenCalled();
    expect(composer).toHaveValue("Composed input");
  });

  it("steers the active response and queues an explicit follow-up", { timeout: 30_000 }, async () => {
    const user = userEvent.setup();
    // A long chunk delay keeps the mock response streaming while the test
    // types and clicks; the delay drops to zero once every queue action is in.
    const bridge = new MockGuruTerminalBridge({ delay_ms: 2_000 });
    const chatSteer = vi.spyOn(bridge, "chatSteer");
    const chatSend = vi.spyOn(bridge, "chatSend");
    await openApp(bridge);

    const composer = screen.getByRole("textbox", { name: "Message Guru" });
    await user.type(composer, "Start the long analysis");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByRole("button", { name: "Stop response" });
    expect(screen.queryByText("Responding")).not.toBeInTheDocument();
    expect(document.querySelector(".composer-running")).toBeTruthy();
    expect(screen.getByRole("checkbox", { name: "Use memory" })).toBeChecked();
    expect(
      screen.getByRole("checkbox", { name: "Update memory" }),
    ).toBeChecked();
    expect(composer).toBeEnabled();
    expect(composer).toHaveAttribute("placeholder", "Ask Guru");
    expect(
      screen.getByRole("button", { name: "Steer current response" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "Queue after current response" }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", {
        name: "Model settings for this message",
      }),
    ).toBeVisible();

    await user.type(composer, "Focus on downside risk");
    await user.click(
      screen.getByRole("button", { name: "Steer current response" }),
    );
    await waitFor(() =>
      expect(chatSteer).toHaveBeenCalledWith(
        expect.objectContaining({
          prompt: "Focus on downside risk",
          thread_id: "thread-margin",
        }),
      ),
    );
    expect(composer).toHaveValue("");
    const conversation = screen.getByLabelText("Conversation");
    expect(within(conversation).getByText("Focus on downside risk")).toBeVisible();
    expect(screen.queryByLabelText("Pending chat instructions")).not.toBeInTheDocument();
    expect(composer).toHaveFocus();

    await user.type(composer, "Also check cash");
    await user.keyboard("{Meta>}{Enter}{/Meta}");
    await waitFor(() => expect(chatSteer).toHaveBeenCalledTimes(2));
    expect(chatSteer).toHaveBeenLastCalledWith(
      expect.objectContaining({
        prompt: "Also check cash",
        thread_id: "thread-margin",
      }),
    );
    expect(composer).toHaveValue("");
    expect(composer).toHaveFocus();
    expect(within(conversation).getByText("Also check cash")).toBeVisible();

    await user.type(composer, "Then summarize the catalysts");
    await user.keyboard("{Enter}");
    expect(composer).toHaveValue("");
    expect(composer).toHaveFocus();
    const pendingQueue = () => screen.getByLabelText("Pending chat instructions");
    expect(within(pendingQueue()).getByText("Queued")).toBeVisible();
    expect(within(pendingQueue()).getByText("Then summarize the catalysts")).toBeVisible();
    expect(
      within(conversation).queryByText("Then summarize the catalysts"),
    ).not.toBeInTheDocument();
    expect(chatSteer).toHaveBeenCalledTimes(2);

    await user.type(composer, "Queue this with the shortcut");
    await user.click(
      screen.getByRole("button", { name: "Queue after current response" }),
    );
    expect(composer).toHaveValue("");
    expect(composer).toHaveFocus();
    expect(within(pendingQueue()).getAllByText("Queued")).toHaveLength(2);
    expect(within(pendingQueue()).getByText("Queue this with the shortcut")).toBeVisible();
    expect(
      within(conversation).queryByText("Queue this with the shortcut"),
    ).not.toBeInTheDocument();
    expect(chatSteer).toHaveBeenCalledTimes(2);
    expect(chatSend).toHaveBeenCalledTimes(1);
    bridge.setStreamDelay(0);
    await waitFor(
      () => expect(chatSend).toHaveBeenCalledTimes(2),
      { timeout: 15_000 },
    );
    expect(chatSend.mock.calls[1]?.[0]).toMatchObject({
      prompt: "Then summarize the catalysts",
    });
    await waitFor(
      () =>
        expect(
          within(conversation).getByText("Then summarize the catalysts"),
        ).toBeVisible(),
      { timeout: 15_000 },
    );
    await waitFor(
      () => expect(chatSend).toHaveBeenCalledTimes(3),
      { timeout: 15_000 },
    );
    expect(chatSend.mock.calls[2]?.[0]).toMatchObject({
      prompt: "Queue this with the shortcut",
    });
    await waitFor(() =>
      expect(
        within(conversation).getByText("Queue this with the shortcut"),
      ).toBeVisible(),
    );
    expect(screen.queryByLabelText("Pending chat instructions")).not.toBeInTheDocument();
  });

  it("keeps queued follow-ups when the active response is stopped", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 2_000 });
    const chatSend = vi.spyOn(bridge, "chatSend");
    await openApp(bridge);

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Start a long analysis",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByRole("button", { name: "Stop response" });
    const composer = screen.getByRole("textbox", { name: "Message Guru" });
    await user.type(composer, "Ask this after the current turn");
    await user.keyboard("{Enter}");
    expect(screen.getByText("Queued")).toBeVisible();
    expect(screen.getByText("Ask this after the current turn")).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Stop response" }));
    expect(
      await screen.findByText("Response stopped. Queued messages were kept."),
    ).toBeVisible();
    expect(screen.getByText("Queued")).toBeVisible();
    expect(screen.getByText("Ask this after the current turn")).toBeVisible();
    expect(chatSend).toHaveBeenCalledTimes(1);
  });

  it("resolves $ Skills, @ plugins, and / as a combined picker without forwarding commands", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    await bridge.marketplaceConnectorConfigure({
      entry_id: "sec.edgar",
      config: { contact_email: "research@example.com" },
    });
    await bridge.guruCapabilityEnable({
      guru_id: "guru-quality",
      entry_id: "sec.edgar",
    });
    const chatSend = vi.spyOn(bridge, "chatSend");
    await openApp(bridge);
    const composer = screen.getByRole("textbox", { name: "Message Guru" });

    const memoryToggle = screen.getByRole("checkbox", { name: "Use memory" });
    await user.click(memoryToggle);
    expect(memoryToggle).not.toBeChecked();
    await user.type(composer, "@");
    expect(
      await screen.findByRole("option", { name: /@sec\.edgar/ }),
    ).toBeVisible();
    expect(
      screen.queryByRole("option", { name: /@guruterminal\.finance-core/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("option", { name: /@guruterminal\.compute-python/ }),
    ).not.toBeInTheDocument();
    await user.type(composer, "sec");
    await user.click(await screen.findByRole("option", { name: /@sec\.edgar/ }));
    expect(composer).toHaveValue("@sec.edgar ");
    expect(memoryToggle).not.toBeChecked();

    await user.clear(composer);
    await user.type(composer, "$res");
    expect(
      await screen.findByRole("option", { name: /\$research/ }),
    ).toBeVisible();
    await user.keyboard("{Enter}");
    expect(composer).toHaveValue("$research ");
    expect(chatSend).not.toHaveBeenCalled();

    await user.clear(composer);
    await user.type(composer, "/wiki");
    expect(
      await screen.findByRole("option", { name: /\$wiki/ }),
    ).toBeVisible();
    await user.keyboard("{Enter}");
    expect(composer).toHaveValue("$wiki ");
    expect(chatSend).not.toHaveBeenCalled();
  });

  it("keeps multiple skill and plugin mentions in the sent prompt", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const chatSend = vi.spyOn(bridge, "chatSend");
    await openApp(bridge);
    const composer = screen.getByRole("textbox", { name: "Message Guru" });
    fireEvent.change(composer, {
      target: {
        value:
          "$research $wiki @guruterminal.finance-core Samsung Electronics",
      },
    });
    await user.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() => expect(chatSend).toHaveBeenCalled());
    expect(chatSend.mock.calls[0]?.[0]).toMatchObject({
      prompt: "$research $wiki @guruterminal.finance-core Samsung Electronics",
    });
  });

  it("locks both memory controls while Wiki or Lens is selected and restores them when removed", async () => {
    const user = userEvent.setup();
    await openApp();
    const composer = screen.getByRole("textbox", { name: "Message Guru" });
    const useMemory = screen.getByRole("checkbox", { name: "Use memory" });
    const updateMemory = screen.getByRole("checkbox", { name: "Update memory" });

    await user.click(useMemory);
    await user.click(updateMemory);
    expect(useMemory).not.toBeChecked();
    expect(updateMemory).not.toBeChecked();

    await user.type(composer, "$len");
    await user.click(
      await screen.findByRole("option", { name: /\$lens/ }),
    );
    expect(composer).toHaveValue("$lens ");
    expect(useMemory).toBeChecked();
    expect(updateMemory).toBeChecked();
    expect(useMemory).toBeDisabled();
    expect(updateMemory).toBeDisabled();

    await user.clear(composer);
    expect(useMemory).toBeEnabled();
    expect(updateMemory).toBeEnabled();
    expect(useMemory).not.toBeChecked();
    expect(updateMemory).not.toBeChecked();
  });

  it("closes the prompt shortcut menu with Escape without changing the draft", async () => {
    const user = userEvent.setup();
    await openApp();
    const composer = screen.getByRole("textbox", { name: "Message Guru" });
    await user.type(composer, "/");
    expect(
      await screen.findByRole("listbox", { name: "Prompt shortcuts" }),
    ).toBeVisible();

    await user.keyboard("{Escape}");
    expect(
      screen.queryByRole("listbox", { name: "Prompt shortcuts" }),
    ).not.toBeInTheDocument();
    expect(composer).toHaveValue("/");
  });

  it("sends the model selected for that message", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    await bridge.providerModels("anthropic");
    const chatSend = vi.spyOn(bridge, "chatSend");
    await openApp(bridge);
    const modelMenu = modelMenuButton();
    await user.click(modelMenu);
    expect(screen.getByRole("menu")).toHaveAttribute("data-side", "top");
    await chooseCatalogOption(user, "Claude Sonnet 4.5");
    expect(modelMenu).toHaveTextContent("Claude Sonnet 4.5 · medium");
    expect(modelMenu).not.toHaveTextContent("Anthropic");
    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Use this model once",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(chatSend).toHaveBeenCalled());
    expect(chatSend.mock.calls[0]?.[0]).toMatchObject({
      model_profile_id: "anthropic/claude-sonnet-4-5",
      thinking_level: "medium",
      run_options: {},
    });
  });

  it("keeps the exact thinking level selected after a completed response", async () => {
    const user = userEvent.setup();
    await openApp(new MockGuruTerminalBridge({ delay_ms: 0 }));
    const modelMenu = modelMenuButton();
    await user.click(modelMenu);
    await chooseCatalogOption(user, "max");
    expect(modelMenu).toHaveTextContent("GPT-5.6 Luna · max");

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Keep max selected for the follow-up",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByText("Response complete.");

    expect(modelMenu).toBeEnabled();
    expect(modelMenu).toHaveTextContent("GPT-5.6 Luna · max");
    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Follow up",
    );
    expect(screen.getByRole("button", { name: "Send" })).toBeEnabled();
  });

  it("sends Pi's Fast performance option separately from thinking", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const chatSend = vi.spyOn(bridge, "chatSend");
    await openApp(bridge);
    const modelMenu = modelMenuButton();
    await user.click(modelMenu);
    await chooseCatalogOption(user, "Fast");

    expect(modelMenu).toHaveTextContent("GPT-5.6 Luna · medium · Fast");
    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Use the fast tier",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(chatSend).toHaveBeenCalled());
    expect(chatSend.mock.calls[0]?.[0]).toMatchObject({
      model_profile_id: "model-test",
      thinking_level: "medium",
      run_options: { performance: "fast" },
    });
  });

  it("attaches files and pasted images to a Chat message", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    await bridge.providerModels("anthropic");
    const chatSend = vi.spyOn(bridge, "chatSend");
    await openApp(bridge);

    await user.click(modelMenuButton());
    await chooseCatalogOption(user, "Claude Sonnet 4.5");

    const notes = new File(["margin bridge"], "notes.txt", {
      type: "text/plain",
    });
    await user.upload(screen.getByLabelText("Upload files"), notes);

    const chart = new File([new Uint8Array([137, 80, 78, 71])], "chart.png", {
      type: "image/png",
    });
    fireEvent.paste(screen.getByRole("textbox", { name: "Message Guru" }), {
      clipboardData: {
        items: [{ kind: "file", getAsFile: () => chart }],
      },
    });

    expect(screen.getByText("notes.txt")).toBeVisible();
    expect(screen.getByText("chart.png")).toBeVisible();
    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Review these inputs",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() => expect(chatSend).toHaveBeenCalled());
    expect(chatSend.mock.calls[0]?.[0]).toMatchObject({
      prompt: "Review these inputs",
      model_profile_id: "anthropic/claude-sonnet-4-5",
      attachments: [
        {
          filename: "notes.txt",
          media_type: "text/plain",
        },
        {
          filename: "chart.png",
          media_type: "image/png",
        },
      ],
    });
    expect(
      await screen.findByRole("button", { name: "Download notes.txt" }),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Download chart.png" }),
    ).toBeVisible();
  });

  it("keeps the draft and image attachment when the selected model rejects images", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const chatSend = vi.spyOn(bridge, "chatSend");
    await openApp(bridge);

    const chart = new File([new Uint8Array([137, 80, 78, 71])], "chart.png", {
      type: "image/png",
    });
    await user.upload(screen.getByLabelText("Upload files"), chart);
    const composer = screen.getByRole("textbox", { name: "Message Guru" });
    await user.type(composer, "Review this chart");
    await user.click(screen.getByRole("button", { name: "Send" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The selected model does not accept images.",
    );
    expect(composer).toHaveValue("Review this chart");
    expect(screen.getByText("chart.png")).toBeVisible();
    expect(chatSend).not.toHaveBeenCalled();
  });

  it("shows the generated title for a new Chat thread", async () => {
    const user = userEvent.setup();
    await openApp(new MockGuruTerminalBridge({ delay_ms: 0 }));

    await user.click(
      screen.getByRole("button", {
        name: "New session for Quality Compounder",
      }),
    );
    await screen.findByRole("heading", { name: "New chat" });
    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "삼성전자 실적 분석해봐",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(
      () =>
        expect(
          screen.getByRole("heading", { name: "삼성전자 실적 분석해봐" }),
        ).toBeVisible(),
      { timeout: 15_000 },
    );
    expect(
      screen.getByRole("button", { name: /삼성전자 실적 분석해봐/ }),
    ).toBeVisible();
  });
});
