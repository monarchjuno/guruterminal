import {
  act,
  fireEvent,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MockGuruTerminalBridge } from "../../bridge";
import { openApp } from "../renderApp";

describe("Guru Terminal · Chat workspace", () => {
  it("traps focus in the narrow workspace dialog and restores the opener", async () => {
    const user = userEvent.setup();
    const matchMedia = vi.mocked(window.matchMedia);
    const previousMatchMedia = matchMedia.getMockImplementation();
    matchMedia.mockImplementation((query: string) => ({
      matches: query === "(max-width: 760px)",
      media: query,
      onchange: null,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    }));

    try {
      await openApp(new MockGuruTerminalBridge({ delay_ms: 0 }));
      const composer = screen.getByRole("textbox", { name: "Message Guru" });
      await user.type(composer, "Create a Markdown document");
      const send = screen.getByRole("button", { name: "Send" });
      await user.click(send);

      const dialog = await screen.findByRole("dialog", {
        name: "Chat workspace panel",
      });
      const background = dialog
        .closest(".app-stage")
        ?.querySelector<HTMLElement>(".app-stage-main");
      expect(background?.inert).toBe(true);
      await waitFor(() =>
        expect(
          within(dialog).getByRole("button", { name: "Close chat workspace" }),
        ).toHaveFocus(),
      );

      composer.focus();
      fireEvent.keyDown(document, { key: "Tab" });
      expect(dialog).toContainElement(document.activeElement as HTMLElement);

      fireEvent.keyDown(window, { key: "Escape" });
      await waitFor(() =>
        expect(
          screen.queryByRole("dialog", { name: "Chat workspace panel" }),
        ).not.toBeInTheDocument(),
      );
      expect(background?.inert).toBe(false);
      await waitFor(() =>
        expect(background).toContainElement(
          document.activeElement as HTMLElement,
        ),
      );
    } finally {
      if (previousMatchMedia) matchMedia.mockImplementation(previousMatchMedia);
    }
  });

  it("opens every answer link in a separate browser tab without adding browser state to agent context", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const chatSend = vi.spyOn(bridge, "chatSend");
    const browserTabOpen = vi.spyOn(bridge, "browserTabOpen");
    const browserTabNavigate = vi.spyOn(bridge, "browserTabNavigate");
    const browserTabHistory = vi.spyOn(bridge, "browserTabHistory");
    const browserTabReload = vi.spyOn(bridge, "browserTabReload");
    const browserTabSetBounds = vi.spyOn(bridge, "browserTabSetBounds");
    await openApp(bridge);

    fireEvent.change(screen.getByRole("textbox", { name: "Message Guru" }), {
      target: {
        value: "Review [Example source](https://example.com/report)",
      },
    });
    await user.click(screen.getByRole("button", { name: "Send" }));

    await screen.findByText("Response complete.");
    const answerLink = await screen.findByRole("link", {
      name: /Example source/,
    });
    await user.click(answerLink);
    await user.click(answerLink);

    let workspace = await screen.findByLabelText("Chat workspace panel");
    await waitFor(() => expect(browserTabOpen).toHaveBeenCalledTimes(2));
    expect(
      within(workspace).getAllByRole("tab", { name: "example.com" }),
    ).toHaveLength(2);

    const activeNativeTab = await browserTabOpen.mock.results[1]!.value;
    const browserViewport = within(workspace).getByLabelText("Browser content");
    let viewportWidth = 520;
    vi.spyOn(browserViewport, "getBoundingClientRect").mockImplementation(
      () => ({
        x: 480,
        y: 160,
        top: 160,
        left: 480,
        right: 480 + viewportWidth,
        bottom: 700,
        width: viewportWidth,
        height: 540,
        toJSON: () => ({}),
      }),
    );
    const separator = within(workspace).getByRole("separator", {
      name: "Resize chat workspace",
    });
    browserTabSetBounds.mockClear();
    fireEvent.pointerDown(separator, { pointerId: 7 });
    expect(separator.setPointerCapture).toHaveBeenCalled();
    expect(workspace).toHaveAttribute("data-resizing", "true");
    viewportWidth = 640;
    fireEvent(window, new Event("resize"));
    await waitFor(() => {
      expect(browserTabSetBounds).toHaveBeenCalledWith({
        tab_id: activeNativeTab.tab_id,
        bounds: { x: 480, y: 160, width: 640, height: 540 },
        visible: true,
      });
    });
    expect(
      browserTabSetBounds.mock.calls
        .map(([request]) => request)
        .filter((request) => request.tab_id === activeNativeTab.tab_id),
    ).not.toContainEqual(expect.objectContaining({ visible: false }));
    fireEvent.pointerUp(window, { pointerId: 7 });
    expect(workspace).not.toHaveAttribute("data-resizing");

    await user.click(
      within(workspace).getByRole("button", { name: "Close chat workspace" }),
    );
    await waitFor(() => {
      expect(
        browserTabSetBounds.mock.calls.some(([request]) => !request.visible),
      ).toBe(true);
    });
    await user.click(
      screen.getByRole("button", { name: "Show chat workspace" }),
    );
    workspace = await screen.findByLabelText("Chat workspace panel");
    expect(
      within(workspace).getAllByRole("tab", { name: "example.com" }),
    ).toHaveLength(2);

    const tabs = within(workspace).getAllByRole("tab", {
      name: "example.com",
    });
    tabs[0]?.focus();
    await user.keyboard("{ArrowRight}");
    expect(tabs[1]).toHaveFocus();

    const address = within(workspace).getByRole("textbox", {
      name: "Web address",
    });
    await user.clear(address);
    await user.type(address, "example.org/next");
    await user.click(within(workspace).getByRole("button", { name: "Open" }));
    await waitFor(() => {
      expect(browserTabNavigate).toHaveBeenCalledWith(
        expect.any(String),
        "https://example.org/next",
      );
    });
    await user.click(
      within(workspace).getByRole("button", { name: "Go back" }),
    );
    await user.click(
      within(workspace).getByRole("button", { name: "Go forward" }),
    );
    await user.click(
      within(workspace).getByRole("button", { name: "Reload page" }),
    );
    expect(browserTabHistory).toHaveBeenCalledWith(expect.any(String), "back");
    expect(browserTabHistory).toHaveBeenCalledWith(
      expect.any(String),
      "forward",
    );
    expect(browserTabReload).toHaveBeenCalledWith(expect.any(String));

    const navigateCalls = browserTabNavigate.mock.calls.length;
    await user.clear(address);
    await user.type(address, "quarterly margin research");
    await user.click(within(workspace).getByRole("button", { name: "Open" }));
    expect(await within(workspace).findByRole("alert")).toHaveTextContent(
      /web address, not a search query/i,
    );
    expect(browserTabNavigate).toHaveBeenCalledTimes(navigateCalls);

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Summarize the conversation without the open page",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(chatSend).toHaveBeenCalledTimes(2));
    const followUpRequest = chatSend.mock.calls[1]?.[0];
    expect(JSON.stringify(followUpRequest)).not.toContain("active_artifact");
    expect(JSON.stringify(followUpRequest)).not.toContain("example.com");
    expect(JSON.stringify(followUpRequest)).not.toContain("example.org");
  });

  it("opens, revises, and isolates thread-scoped Markdown artifacts", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const chatSend = vi.spyOn(bridge, "chatSend");
    const chatArtifactRead = vi.spyOn(bridge, "chatArtifactRead");
    await openApp(bridge);

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Create a Markdown document",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));

    const viewer = await screen.findByLabelText("Chat workspace panel");
    expect(viewer).toHaveRole("complementary");
    expect(viewer).not.toHaveAttribute("aria-modal");
    expect(
      viewer.closest(".app-stage")?.querySelector(".app-stage-main"),
    ).not.toHaveAttribute("inert");
    expect(
      within(viewer).getByRole("heading", { name: "Research note" }),
    ).toBeVisible();
    expect(
      within(viewer).queryByText("Chat workspace"),
    ).not.toBeInTheDocument();
    const tabbar = viewer.querySelector(".workspace-tabbar");
    const titlebar = viewer.querySelector(".artifact-panel-header");
    expect(
      tabbar &&
        titlebar &&
        tabbar.compareDocumentPosition(titlebar) &
          Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(
      within(viewer).getByRole("tabpanel", { name: "Research note" }),
    ).toBeVisible();
    expect(within(viewer).queryByText("Research tabs")).not.toBeInTheDocument();
    expect(
      within(viewer).queryByRole("button", { name: "Use in chat" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("Context for next message"),
    ).not.toBeInTheDocument();
    expect(JSON.stringify(chatSend.mock.calls[0]?.[0])).not.toContain(
      "active_artifact",
    );

    const stage = viewer.closest(".app-stage") as HTMLElement;
    let stageWidth = 1_000;
    vi.spyOn(stage, "getBoundingClientRect").mockImplementation(() => ({
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: stageWidth,
      bottom: 700,
      width: stageWidth,
      height: 700,
      toJSON: () => ({}),
    }));
    let separator = within(viewer).getByRole("separator", {
      name: "Resize chat workspace",
    });
    expect(separator).toHaveAttribute("aria-orientation", "vertical");
    fireEvent.pointerDown(separator, { pointerId: 1 });
    fireEvent(
      window,
      new MouseEvent("pointermove", { bubbles: true, clientX: 400 }),
    );
    fireEvent(window, new MouseEvent("pointerup", { bubbles: true }));
    await waitFor(() =>
      expect(separator).toHaveAttribute("aria-valuenow", "580"),
    );
    stageWidth = 1_800;
    fireEvent.keyDown(separator, { key: "End" });
    expect(separator).toHaveAttribute("aria-valuemax", "1280");
    expect(separator).toHaveAttribute("aria-valuenow", "1280");

    await user.click(
      screen.getByRole("button", { name: "Show workspace below chat" }),
    );
    expect(viewer).toHaveAttribute("data-placement", "bottom");
    separator = within(viewer).getByRole("separator", {
      name: "Resize chat workspace",
    });
    expect(separator).toHaveAttribute("aria-orientation", "horizontal");
    fireEvent.keyDown(separator, { key: "ArrowUp" });
    expect(separator).toHaveAttribute("aria-valuenow", "436");

    await user.click(
      screen.getByRole("button", { name: "Maximize chat workspace" }),
    );
    expect(viewer).toHaveAttribute("data-maximized", "true");
    expect(
      viewer.closest(".app-stage")?.querySelector(".app-stage-main"),
    ).not.toBeVisible();
    expect(
      within(viewer).queryByRole("separator", {
        name: "Resize chat workspace",
      }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Restore chat workspace" }),
    ).toBeVisible();

    await user.click(
      screen.getByRole("button", { name: "Restore chat workspace" }),
    );
    expect(viewer).not.toHaveAttribute("data-maximized");
    expect(
      viewer.closest(".app-stage")?.querySelector(".app-stage-main"),
    ).toBeVisible();
    expect(
      within(viewer).getByRole("separator", { name: "Resize chat workspace" }),
    ).toHaveAttribute("aria-orientation", "horizontal");

    const composer = screen.getByRole("textbox", { name: "Message Guru" });
    await user.type(composer, "Update the Research note with another section");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(chatSend).toHaveBeenCalledTimes(2));
    await act(async () => {
      await chatSend.mock.results[1]!.value;
    });
    expect(chatSend.mock.calls[1]?.[0].prompt).toContain("Research note");
    expect(JSON.stringify(chatSend.mock.calls[1]?.[0])).not.toContain(
      "active_artifact",
    );
    expect(
      document.querySelector(".artifact-context-chip"),
    ).not.toBeInTheDocument();
    expect(viewer).not.toHaveTextContent(/Version|History|Current version/);

    await user.click(
      screen.getAllByRole("button", {
        name: /Open document Research note/,
      })[0]!,
    );
    await waitFor(() =>
      expect(viewer).not.toHaveTextContent(/Version|History/),
    );
    expect(
      document.querySelector(".artifact-context-chip"),
    ).not.toBeInTheDocument();
    expect(viewer).toBeVisible();

    await user.keyboard("{Escape}");
    expect(
      screen.queryByLabelText("Chat workspace panel"),
    ).not.toBeInTheDocument();
    await user.click(
      screen.getAllByRole("button", { name: /Open document/ })[0]!,
    );
    expect(await screen.findByLabelText("Chat workspace panel")).toBeVisible();
    await waitFor(() =>
      expect(chatArtifactRead).toHaveBeenLastCalledWith(
        expect.any(String),
        expect.any(String),
        expect.any(String),
      ),
    );
    expect(
      screen.queryByText("Context for next message"),
    ).not.toBeInTheDocument();

    await user.click(
      screen.getByRole("button", {
        name: "New session for Quality Compounder",
      }),
    );
    await screen.findByRole("heading", { name: "New chat" });
    expect(
      document.querySelector(".artifact-context-chip"),
    ).not.toBeInTheDocument();
  });
});
