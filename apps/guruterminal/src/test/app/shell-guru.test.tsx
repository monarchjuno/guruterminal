import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../../App";
import { MockGuruTerminalBridge } from "../../bridge";
import {
  createMockGuruCapabilityBindings,
  createMockMarketplaceSnapshot,
} from "../../marketplace/mockSnapshot";
import type { ChatThread, GuruSummary, ModelCatalog } from "../../types";
import { chooseGuru, mainTab, openApp } from "../renderApp";

describe("Guru Terminal · Shell and Guru", () => {
  it("recovers an interrupted Memory update from one simple Guru-scoped screen", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const [firstGuru] = await bridge.guruList();
    const gurus: GuruSummary[] = [
      {
        ...firstGuru,
        availability: {
          status: "recovery_required",
          reason: "interrupted_memory_update",
          action: "recover_memory",
        },
      },
    ];
    vi.spyOn(bridge, "guruList").mockResolvedValueOnce(gurus);
    const recover = vi.spyOn(bridge, "guruRecover");

    render(<App bridge={bridge} />);

    expect(
      await screen.findByRole("heading", { name: "Memory needs recovery" }),
    ).toBeVisible();
    expect(screen.queryByText("Guru is unavailable until")).not.toBeInTheDocument();

    await user.click(mainTab(/Marketplace/));
    expect(
      await screen.findByRole("heading", { name: "Marketplace" }),
    ).toBeVisible();
    await user.click(mainTab(/Chat/));
    await screen.findByRole("heading", { name: "Memory needs recovery" });
    await user.click(screen.getByRole("button", { name: "Settings" }));
    expect(
      await screen.findByRole("heading", { name: "Settings" }),
    ).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Back to app" }));

    await user.click(screen.getByRole("button", { name: "Recover memory" }));

    await waitFor(() =>
      expect(recover).toHaveBeenCalledWith({
        guru_id: gurus[0].id,
        action: "recover_memory",
      }),
    );
    expect(
      await screen.findByRole("heading", {
        name: "How should we read the margin decline?",
      }),
    ).toBeVisible();
  });

  it("does not select a recovered Guru after the user moves to another Guru", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const listed = await bridge.guruList();
    const recoveringGuru: GuruSummary = {
      ...listed[0],
      availability: {
        status: "recovery_required",
        reason: "interrupted_memory_update",
        action: "recover_memory",
      },
    };
    vi.spyOn(bridge, "guruList").mockResolvedValueOnce([
      recoveringGuru,
      ...listed.slice(1),
    ]);

    let finishRecovery: ((guru: GuruSummary) => void) | undefined;
    vi.spyOn(bridge, "guruRecover").mockImplementation(
      () =>
        new Promise((resolve) => {
          finishRecovery = resolve;
        }),
    );

    render(<App bridge={bridge} />);
    await screen.findByRole("heading", { name: "Downside scenario review" });
    await chooseGuru(user, recoveringGuru.name);
    await screen.findByRole("heading", { name: "Memory needs recovery" });
    await user.click(screen.getByRole("button", { name: "Recover memory" }));
    await chooseGuru(user, "Cycle Reader");

    await act(async () => {
      finishRecovery?.({
        ...recoveringGuru,
        availability: { status: "available" },
      });
    });

    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Cycle Reader" }),
      ).toHaveAttribute("aria-current", "page"),
    );
  });

  it("keeps Agent navigation and creation available during recovery", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const listed = await bridge.guruList();
    const recoveringGuru: GuruSummary = {
      ...listed[0],
      availability: {
        status: "recovery_required",
        reason: "interrupted_memory_update",
        action: "recover_memory",
      },
    };
    vi.spyOn(bridge, "guruList").mockResolvedValueOnce([
      recoveringGuru,
      ...listed.slice(1),
    ]);
    const skills = vi.spyOn(bridge, "agentSkillCatalog");

    render(<App bridge={bridge} />);
    await screen.findByRole("heading", { name: "Downside scenario review" });
    await user.click(mainTab(/Agents/));
    await user.click(
      within(screen.getByRole("complementary", { name: "Agents" })).getByRole(
        "button",
        { name: /Quality Compounder/ },
      ),
    );

    expect(
      await screen.findByRole("heading", { name: "Memory needs recovery" }),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "New agent" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Import" })).toBeVisible();
    expect(
      screen.getByRole("button", { name: /Contrarian Value/ }),
    ).toBeVisible();
    expect(skills).not.toHaveBeenCalledWith(recoveringGuru.id);
  });

  it("mounts workspace tools only after their first visit", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const librarySearch = vi.spyOn(bridge, "librarySearch");

    await openApp(bridge);
    const initialSearchCount = librarySearch.mock.calls.length;
    await user.click(mainTab(/Memory/));
    await screen.findByRole("heading", { name: "Memory", level: 1 });
    await waitFor(() =>
      expect(librarySearch.mock.calls.length).toBeGreaterThan(
        initialSearchCount,
      ),
    );
  });

  it("persists Light, Dark, and System appearance preferences", async () => {
    const user = userEvent.setup();
    const listeners = new Set<(event: MediaQueryListEvent) => void>();
    let prefersDark = false;
    const originalMatchMedia = window.matchMedia;

    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        get matches() {
          return prefersDark;
        },
        media: query,
        onchange: null,
        addEventListener: (
          _type: string,
          listener: (event: MediaQueryListEvent) => void,
        ) => listeners.add(listener),
        removeEventListener: (
          _type: string,
          listener: (event: MediaQueryListEvent) => void,
        ) => listeners.delete(listener),
        addListener: vi.fn(),
        removeListener: vi.fn(),
        dispatchEvent: vi.fn(),
      })),
    });
    window.localStorage.removeItem("guruterminal:theme:v1");

    const view = render(
      <App bridge={new MockGuruTerminalBridge({ delay_ms: 0 })} />,
    );
    try {
      await screen.findByRole("heading", {
        name: "How should we read the margin decline?",
      });
      await user.click(screen.getByRole("button", { name: "Settings" }));
      await user.click(screen.getByRole("button", { name: "Appearance" }));

      const light = screen.getByRole("button", { name: "Light theme" });
      const dark = screen.getByRole("button", { name: "Dark theme" });
      const system = screen.getByRole("button", { name: "System theme" });
      expect(system).toHaveAttribute("aria-pressed", "true");
      expect(document.documentElement).toHaveAttribute("data-theme", "light");

      await user.click(dark);
      expect(dark).toHaveAttribute("aria-pressed", "true");
      expect(document.documentElement).toHaveAttribute("data-theme", "dark");
      expect(window.localStorage.getItem("guruterminal:theme:v1")).toBe("dark");

      await user.click(light);
      expect(light).toHaveAttribute("aria-pressed", "true");
      expect(document.documentElement).toHaveAttribute("data-theme", "light");
      expect(window.localStorage.getItem("guruterminal:theme:v1")).toBe(
        "light",
      );

      prefersDark = true;
      await act(async () => {
        listeners.forEach((listener) =>
          listener({ matches: true } as MediaQueryListEvent),
        );
      });
      await user.click(system);
      expect(system).toHaveAttribute("aria-pressed", "true");
      expect(document.documentElement).toHaveAttribute("data-theme", "dark");
      expect(window.localStorage.getItem("guruterminal:theme:v1")).toBe(
        "system",
      );
    } finally {
      view.unmount();
      Object.defineProperty(window, "matchMedia", {
        configurable: true,
        value: originalMatchMedia,
      });
      window.localStorage.removeItem("guruterminal:theme:v1");
      delete document.documentElement.dataset.theme;
      document.documentElement.style.removeProperty("color-scheme");
    }
  });

  it("keeps the primary views above the Guru list and its Chat history", async () => {
    await openApp();
    expect(document.querySelector(".app-brand-mark")).not.toBeInTheDocument();
    const navigationHeader = screen
      .getByText("Guru Terminal")
      .closest('[data-slot="sidebar-header"]');
    expect(navigationHeader).toBeVisible();
    expect(navigationHeader).not.toHaveClass("border-b");
    expect(navigationHeader).toHaveAttribute("data-tauri-drag-region", "deep");
    expect(document.querySelector(".app-header")).toHaveAttribute(
      "data-tauri-drag-region",
      "deep",
    );
    expect(mainTab(/Chat/)).toHaveAttribute("aria-current", "page");
    expect(screen.queryByRole("button", { name: /Training/ })).not.toBeInTheDocument();
    expect(mainTab(/Memory/)).toBeVisible();
    expect(mainTab(/Agents/)).toBeVisible();
    expect(mainTab(/Marketplace/)).toBeVisible();
    expect(screen.getByRole("button", { name: "Settings" })).toHaveAttribute(
      "data-size",
      "sm",
    );

    const guruNavigation = screen.getByRole("navigation", { name: "Gurus" });
    expect(
      within(guruNavigation).getByRole("button", {
        name: "Quality Compounder",
      }),
    ).toHaveAttribute("aria-current", "page");
    expect(
      within(guruNavigation).getByRole("button", { name: "Contrarian Value" }),
    ).toBeVisible();
    expect(
      within(guruNavigation).getByRole("button", { name: "Cycle Reader" }),
    ).toBeVisible();
    expect(
      within(guruNavigation).getByRole("button", {
        name: "How should we read the margin decline?",
      }),
    ).toBeVisible();
    expect(
      screen.queryByRole("combobox", { name: "Select Guru" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "New chat" }),
    ).not.toBeInTheDocument();
    expect(
      within(guruNavigation).getByRole("button", {
        name: "New session for Quality Compounder",
      }),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: "New agent" }),
    ).toBeVisible();
  });

  it("creates an agent from the Chat sidebar Agents heading", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const createGuru = vi.spyOn(bridge, "guruCreate");

    await user.click(screen.getByRole("button", { name: "New agent" }));
    expect(
      await screen.findByRole("dialog", { name: "Create agent" }),
    ).toBeVisible();
    await user.type(screen.getByRole("textbox", { name: "Name" }), "Sidebar Guru");
    await user.click(screen.getByRole("button", { name: "Create agent" }));

    await waitFor(() =>
      expect(createGuru).toHaveBeenCalledWith({ name: "Sidebar Guru" }),
    );
    expect(
      await screen.findByRole("button", { name: "Sidebar Guru" }),
    ).toHaveAttribute("aria-current", "page");
    expect(mainTab(/Chat/)).toHaveAttribute("aria-current", "page");
  });

  it("keeps the macOS titlebar sidebar toggle available while the sidebar collapses", async () => {
    const user = userEvent.setup();
    await openApp();

    const titlebarTrigger = screen.getByRole("button", {
      name: "Show or hide sidebar",
    });
    expect(titlebarTrigger).toHaveClass("macos-titlebar-sidebar-trigger");
    await user.click(titlebarTrigger);

    expect(
      document.querySelector('[data-slot="sidebar"][data-state="collapsed"]'),
    ).toBeInTheDocument();
    expect(titlebarTrigger).toBeInTheDocument();
  });

  it("collapses and expands the selected Guru session list", async () => {
    const user = userEvent.setup();
    await openApp();

    const session = screen.getByRole("button", {
      name: "How should we read the margin decline?",
    });
    const agent = screen.getByRole("button", { name: "Quality Compounder" });
    expect(agent).toHaveAttribute("aria-expanded", "true");
    expect(session).toBeVisible();

    await user.click(agent);

    expect(
      screen.queryByRole("button", {
        name: "How should we read the margin decline?",
      }),
    ).not.toBeInTheDocument();
    expect(agent).toHaveAttribute("aria-expanded", "false");

    await user.click(agent);

    expect(
      screen.getByRole("button", {
        name: "How should we read the margin decline?",
      }),
    ).toBeVisible();
  });

  it("opens a draft even when the Guru list was collapsed", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const create = vi.spyOn(bridge, "chatCreate");

    await user.click(screen.getByRole("button", { name: "Quality Compounder" }));
    expect(
      screen.queryByRole("button", {
        name: "How should we read the margin decline?",
      }),
    ).not.toBeInTheDocument();

    await user.click(
      screen.getByRole("button", {
        name: "New session for Quality Compounder",
      }),
    );
    expect(
      await screen.findByRole("heading", { name: "New chat" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "New chat" }),
    ).not.toBeInTheDocument();
    expect(create).not.toHaveBeenCalled();
    expect(
      screen.getByRole("button", {
        name: "How should we read the margin decline?",
      }),
    ).toBeVisible();
  });

  it("does not advertise an expanded session list when the Guru has no threads", async () => {
    const user = userEvent.setup();
    await openApp();

    await user.click(screen.getByRole("button", { name: "New agent" }));
    await user.type(screen.getByRole("textbox", { name: "Name" }), "Empty Guru");
    await user.click(screen.getByRole("button", { name: "Create agent" }));

    const created = await screen.findByRole("button", { name: "Empty Guru" });
    expect(created).toHaveAttribute("aria-current", "page");
    expect(created).not.toHaveAttribute("aria-expanded");
  });

  it("replaces the app sidebar with Settings navigation", async () => {
    const user = userEvent.setup();
    await openApp();

    await user.click(screen.getByRole("button", { name: "Settings" }));

    expect(
      screen.getByRole("navigation", { name: "Settings sections" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("navigation", { name: "Main views" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("tablist", { name: "Settings sections" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Model" })).toHaveAttribute(
      "aria-current",
      "page",
    );

    await user.click(screen.getByRole("button", { name: "Appearance" }));
    expect(screen.getByRole("button", { name: "Appearance" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    expect(screen.getByRole("button", { name: "Dark theme" })).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Back to app" }));
    expect(
      screen.getByRole("navigation", { name: "Main views" }),
    ).toBeVisible();
    expect(mainTab(/Chat/)).toHaveAttribute("aria-current", "page");
  });

  it("creates, renames, and deletes sessions from the Guru list", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const create = vi.spyOn(bridge, "chatCreate");
    const rename = vi.spyOn(bridge, "chatRename");
    const remove = vi.spyOn(bridge, "chatDelete");

    await user.click(
      screen.getByRole("button", {
        name: "New session for Contrarian Value",
      }),
    );
    expect(create).not.toHaveBeenCalled();
    await screen.findByRole("heading", { name: "New chat" });
    expect(
      screen.queryByRole("button", { name: "New chat" }),
    ).not.toBeInTheDocument();

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Downside follow-up",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() =>
      expect(create).toHaveBeenCalledWith({ guru_id: "guru-value" }),
    );
    const newThread = await screen.findByRole("button", {
      name: "Downside follow-up",
    });
    const newThreadRow = newThread.closest("li");
    expect(newThreadRow).not.toBeNull();
    await user.click(
      within(newThreadRow!).getByRole("button", { name: "Rename session" }),
    );
    const name = screen.getByRole("textbox", { name: "Name" });
    await user.clear(name);
    await user.type(name, "Renamed follow-up");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(rename).toHaveBeenCalledWith({
        guru_id: "guru-value",
        thread_id: expect.any(String),
        title: "Renamed follow-up",
      }),
    );
    expect(
      screen.getByRole("button", { name: "Renamed follow-up" }),
    ).toBeVisible();

    const renamedThread = screen.getByRole("button", {
      name: "Renamed follow-up",
    });
    const renamedThreadRow = renamedThread.closest("li");
    expect(renamedThreadRow).not.toBeNull();
    await user.click(
      within(renamedThreadRow!).getByRole("button", {
        name: "Delete session",
      }),
    );
    expect(
      screen.getByRole("heading", { name: "Delete session?" }),
    ).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() =>
      expect(remove).toHaveBeenCalledWith({
        guru_id: "guru-value",
        thread_id: expect.any(String),
      }),
    );
    expect(
      screen.queryByRole("button", { name: "Renamed follow-up" }),
    ).not.toBeInTheDocument();
  });

  it("discovers Pi models after saving a provider-level API key", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const configureProvider = vi.spyOn(bridge, "providerConfigure");

    await user.click(screen.getByRole("button", { name: "Settings" }));
    await screen.findByRole("switch", { name: "Show openai default in Chat" });
    await user.click(screen.getByRole("button", { name: "Connect provider" }));
    await user.click(screen.getByRole("combobox", { name: "Provider" }));
    await user.click(await screen.findByRole("option", { name: /Anthropic/ }));
    await user.type(
      screen.getByLabelText("Anthropic API key"),
      "write-only-secret",
    );
    await user.click(
      await screen.findByRole("button", { name: "Connect and load models" }),
    );

    await waitFor(() => {
      expect(configureProvider).toHaveBeenCalledWith({
        provider: "anthropic",
        api_key: "write-only-secret",
      });
    });
    expect(
      await screen.findByRole("switch", {
        name: "Show Claude Sonnet 4.5 in Chat",
      }),
    ).toBeChecked();
  });

  it("admits only one API-key provider operation from a rapid double click", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const catalog = await bridge.modelCatalogGet();
    let finishConfigure: ((catalog: ModelCatalog) => void) | undefined;
    const configureProvider = vi
      .spyOn(bridge, "providerConfigure")
      .mockImplementation(
        () =>
          new Promise((resolve) => {
            finishConfigure = resolve;
          }),
      );

    await user.click(screen.getByRole("button", { name: "Settings" }));
    await screen.findByRole("switch", { name: "Show openai default in Chat" });
    await user.click(screen.getByRole("button", { name: "Connect provider" }));
    await user.click(screen.getByRole("combobox", { name: "Provider" }));
    await user.click(await screen.findByRole("option", { name: /Anthropic/ }));
    await user.type(
      screen.getByLabelText("Anthropic API key"),
      "write-only-secret",
    );
    await user.dblClick(
      await screen.findByRole("button", { name: "Connect and load models" }),
    );

    expect(configureProvider).toHaveBeenCalledTimes(1);
    await act(async () => finishConfigure?.(catalog));
  });

  it("connects OpenAI with OAuth before choosing a Pi-discovered model", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const connect = vi.spyOn(bridge, "providerConnect");

    await user.click(screen.getByRole("button", { name: "Settings" }));
    await screen.findByRole("switch", { name: "Show openai default in Chat" });
    await user.click(screen.getByRole("button", { name: "Connect provider" }));
    await user.click(screen.getByRole("combobox", { name: "Provider" }));
    await user.click(
      await screen.findByRole("option", {
        name: /OpenAI with ChatGPT · Recommended/,
      }),
    );
    await user.click(
      screen.getByRole("button", { name: "Continue with ChatGPT" }),
    );

    await waitFor(() =>
      expect(connect).toHaveBeenCalledWith(
        "openai-codex",
        expect.any(Function),
      ),
    );
    expect(
      await screen.findByRole("switch", { name: "Show GPT-5.6 Sol in Chat" }),
    ).toBeChecked();
  });

  it("cancels an abandoned browser sign-in and allows a clean retry", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    let rejectConnect:
      | ((reason: { code: string; message: string }) => void)
      | undefined;
    vi.spyOn(bridge, "providerConnect").mockImplementation(
      (_provider, observer) => {
        observer({
          type: "opening_browser",
          message: "A secure sign-in page was opened.",
        });
        return new Promise((_resolve, reject) => {
          rejectConnect = reject;
        });
      },
    );
    const cancel = vi
      .spyOn(bridge, "providerConnectCancel")
      .mockImplementation(async () => {
        rejectConnect?.({
          code: "cancelled",
          message: "Provider sign-in was cancelled.",
        });
      });
    const openBrowser = vi
      .spyOn(bridge, "providerConnectOpenBrowser")
      .mockResolvedValue();

    await user.click(screen.getByRole("button", { name: "Settings" }));
    await screen.findByRole("switch", { name: "Show openai default in Chat" });
    await user.click(screen.getByRole("button", { name: "Connect provider" }));
    await user.click(screen.getByRole("combobox", { name: "Provider" }));
    await user.click(
      await screen.findByRole("option", {
        name: /OpenAI with ChatGPT · Recommended/,
      }),
    );
    await user.click(
      screen.getByRole("button", { name: "Continue with ChatGPT" }),
    );

    expect(await screen.findByText("Finish signing in")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Open sign-in page" }));
    await waitFor(() => expect(openBrowser).toHaveBeenCalledTimes(1));
    expect(
      await screen.findByText("Sign-in page requested. Continue in your browser."),
    ).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Cancel sign-in" }));
    await waitFor(() => expect(cancel).toHaveBeenCalledTimes(1));
    expect(
      await screen.findByText("Sign-in cancelled. You can try again."),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Continue with ChatGPT" }),
    ).toBeEnabled();
  });

  it("shows the backend provider error instead of hiding it behind a fallback", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    vi.spyOn(bridge, "providerConnect").mockRejectedValue({
      code: "internal",
      message: "OpenAI sign-in could not open the trusted authorization page",
    });

    await user.click(screen.getByRole("button", { name: "Settings" }));
    await screen.findByRole("switch", { name: "Show openai default in Chat" });
    await user.click(screen.getByRole("button", { name: "Connect provider" }));
    await user.click(screen.getByRole("combobox", { name: "Provider" }));
    await user.click(
      await screen.findByRole("option", {
        name: /OpenAI with ChatGPT · Recommended/,
      }),
    );
    await user.click(
      screen.getByRole("button", { name: "Continue with ChatGPT" }),
    );

    expect(
      await screen.findByText(
        "OpenAI sign-in could not open the trusted authorization page",
      ),
    ).toBeVisible();
  });

  it("offers subscription sign-in and an API key on the same provider", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const connect = vi.spyOn(bridge, "providerConnect");

    await user.click(screen.getByRole("button", { name: "Settings" }));
    await screen.findByRole("switch", { name: "Show openai default in Chat" });
    await user.click(screen.getByRole("button", { name: "Connect provider" }));
    await user.click(screen.getByRole("combobox", { name: "Provider" }));
    await user.click(await screen.findByRole("option", { name: /^xAI$/ }));

    expect(
      screen.getByRole("button", { name: "Continue with SuperGrok" }),
    ).toBeVisible();
    expect(screen.getByLabelText("xAI API key")).toBeVisible();

    await user.click(
      screen.getByRole("button", { name: "Continue with SuperGrok" }),
    );
    await waitFor(() =>
      expect(connect).toHaveBeenCalledWith("xai", expect.any(Function)),
    );
  });

  it("shows connected providers and disconnects a saved credential", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const disconnect = vi.spyOn(bridge, "providerDisconnect");

    await user.click(screen.getByRole("button", { name: "Settings" }));
    const connected = await screen.findByRole("list", {
      name: "Connected providers",
    });
    expect(within(connected).getByText("OpenAI API")).toBeVisible();
    expect(
      await within(connected).findByRole("switch", {
        name: "Show GPT-5.6 Luna in Chat",
      }),
    ).toBeVisible();
    expect(
      await screen.findByRole("switch", {
        name: "Show openai default in Chat",
      }),
    ).toBeVisible();

    await user.click(
      screen.getByRole("button", { name: "Disconnect OpenAI API" }),
    );
    await user.click(screen.getByRole("button", { name: "Disconnect" }));
    await waitFor(() => expect(disconnect).toHaveBeenCalledWith("openai"));
    expect(
      screen.queryByRole("button", { name: "Disconnect OpenAI API" }),
    ).not.toBeInTheDocument();
  });

  it("discovers models when an empty connected provider is opened", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const loadModels = vi.spyOn(bridge, "providerModels");

    await user.click(screen.getByRole("button", { name: "Settings" }));

    await waitFor(() => expect(loadModels).toHaveBeenCalledWith("openai"));
    expect(
      await screen.findByRole("switch", {
        name: "Show openai default in Chat",
      }),
    ).toBeChecked();
  });

  it("hides unchecked provider models from the Chat model picker", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const updateVisibility = vi.spyOn(bridge, "modelVisibilityUpdate");

    await user.click(screen.getByRole("button", { name: "Settings" }));
    const luna = await screen.findByRole("switch", {
      name: "Show GPT-5.6 Luna in Chat",
    });
    expect(luna).toBeChecked();
    await user.click(luna);
    const openaiDefault = await screen.findByRole("switch", {
      name: "Show openai default in Chat",
    });
    await user.click(openaiDefault);

    await waitFor(() =>
      expect(updateVisibility).toHaveBeenCalledWith({
        model_profile_id: "model-test",
        visible_in_chat: false,
      }),
    );
    expect(luna).not.toBeChecked();
    expect(openaiDefault).not.toBeChecked();
    await user.click(screen.getByRole("button", { name: "Back to app" }));

    expect(
      await screen.findByRole("heading", { name: "No models in Chat" }),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Open Settings" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", {
        name: "Model settings for this message",
      }),
    ).not.toBeInTheDocument();
  });

  it("falls back to another visible model after the preferred model is hidden", async () => {
    const user = userEvent.setup();
    await openApp();

    await user.click(screen.getByRole("button", { name: "Settings" }));
    await screen.findByRole("switch", {
      name: "Show openai default in Chat",
    });
    await user.click(
      screen.getByRole("switch", { name: "Show GPT-5.6 Luna in Chat" }),
    );
    await user.click(screen.getByRole("button", { name: "Back to app" }));

    const modelMenu = await screen.findByRole("button", {
      name: "Model settings for this message",
    });
    expect(modelMenu).toBeEnabled();
    expect(modelMenu).toHaveTextContent("openai default");
  });

  it("lists a newly connected OAuth provider in the connected overview", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const disconnect = vi.spyOn(bridge, "providerDisconnect");

    await user.click(screen.getByRole("button", { name: "Settings" }));
    await screen.findByRole("switch", { name: "Show openai default in Chat" });
    await user.click(screen.getByRole("button", { name: "Connect provider" }));
    await user.click(screen.getByRole("combobox", { name: "Provider" }));
    await user.click(await screen.findByRole("option", { name: /^xAI$/ }));
    await user.click(
      screen.getByRole("button", { name: "Continue with SuperGrok" }),
    );
    expect(
      await screen.findByRole("switch", { name: /Show xai default in Chat/i }),
    ).toBeChecked();

    const connected = screen.getByRole("list", { name: "Connected providers" });
    expect(within(connected).getByText("xAI")).toBeVisible();
    expect(
      within(connected).getByRole("button", {
        name: "Disconnect xAI",
      }),
    ).toBeVisible();
    await user.click(
      within(connected).getByRole("button", {
        name: "Disconnect xAI",
      }),
    );
    await user.click(screen.getByRole("button", { name: "Disconnect" }));
    await waitFor(() => expect(disconnect).toHaveBeenCalledWith("xai"));
    expect(
      screen.queryByRole("button", { name: "Disconnect xAI" }),
    ).not.toBeInTheDocument();
  });

  it("configures Agent skills and installed tools from Agents", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const updateSkills = vi.spyOn(bridge, "agentSkillsUpdate");
    const disableTool = vi.spyOn(bridge, "guruCapabilityDisable");

    await user.click(mainTab(/Agents/));
    expect(
      screen.queryByRole("navigation", { name: "Gurus" }),
    ).not.toBeInTheDocument();
    const wiki = await screen.findByRole("switch", {
      name: "Wiki: enabled",
    });
    const lens = screen.getByRole("switch", {
      name: "Lens: enabled",
    });
    expect(lens).toBeEnabled();
    expect(screen.queryByText("Built in")).not.toBeInTheDocument();

    await user.click(wiki);
    await waitFor(() =>
      expect(updateSkills).toHaveBeenCalledWith({
        guru_id: "guru-quality",
        skill_ids: ["research", "lens"],
      }),
    );

    await user.click(
      screen.getByRole("switch", { name: "Finance Core: enabled" }),
    );
    await waitFor(() =>
      expect(disableTool).toHaveBeenCalledWith({
        guru_id: "guru-quality",
        entry_id: "guruterminal.finance-core",
      }),
    );

    await user.click(
      screen.getByRole("button", { name: "Browse Marketplace" }),
    );
    expect(
      await screen.findByRole("heading", { name: "Marketplace" }),
    ).toBeVisible();
  });

  it("renders the authoritative Tool binding returned by native", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();

    await user.click(mainTab(/Agents/));
    await user.click(
      await screen.findByRole("switch", { name: "Finance Core: enabled" }),
    );
    const enable = vi.spyOn(bridge, "guruCapabilityEnable").mockResolvedValue({
      entry_id: "guruterminal.finance-core",
      enabled: false,
      granted_permissions: [],
      available: true,
    });

    await user.click(
      screen.getByRole("switch", { name: "Finance Core: disabled" }),
    );

    await waitFor(() =>
      expect(enable).toHaveBeenCalledWith({
        guru_id: "guru-quality",
        entry_id: "guruterminal.finance-core",
      }),
    );
    expect(
      screen.getByRole("switch", { name: "Finance Core: disabled" }),
    ).not.toBeChecked();
  });

  it("does not present an unavailable connector as enabled for the Agent", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const bindings = await bridge.guruCapabilityList("guru-quality");
    vi.spyOn(bridge, "guruCapabilityList").mockResolvedValue(
      bindings.map((binding) =>
        binding.entry_id === "koreainvestment.market-data"
          ? { ...binding, enabled: true, available: false }
          : binding,
      ),
    );

    await user.click(mainTab(/Agents/));

    const kis = await screen.findByRole("switch", {
      name: "Korea Investment Open Trading API: disabled",
    });
    expect(kis).toBeDisabled();
    expect(
      screen.getAllByText("Set up in Marketplace").length,
    ).toBeGreaterThan(0);
  });

  it("does not send a missing bundled runtime to Marketplace setup", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const snapshot = createMockMarketplaceSnapshot(
      new Set(),
      new Set(),
      new Map(),
      new Map(),
      new Map(),
      new Set(["openbb.platform"]),
    );
    const bindings = createMockGuruCapabilityBindings(
      new Set(),
      new Set(),
      snapshot,
    );
    vi.spyOn(bridge, "marketplaceSnapshot").mockResolvedValue(snapshot);
    vi.spyOn(bridge, "guruCapabilityList").mockResolvedValue(bindings);

    await user.click(mainTab(/Agents/));

    const openbb = await screen.findByRole("switch", {
      name: "OpenBB Platform: disabled",
    });
    expect(openbb).toBeDisabled();
    expect(
      screen.getByText("Bundled runtime is missing from this build"),
    ).toBeVisible();
    const openbbRow = openbb.closest(".agent-capability");
    expect(openbbRow).not.toBeNull();
    expect(
      within(openbbRow as HTMLElement).queryByText("Set up in Marketplace"),
    ).toBeNull();
  });

  it("imports, renames, exports, and deletes an Agent with stable selection", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const importMemory = vi.spyOn(bridge, "guruImportMemory");
    const rename = vi.spyOn(bridge, "guruRename");
    const exportMemory = vi.spyOn(bridge, "guruExportMemory");
    const remove = vi.spyOn(bridge, "guruDelete");

    await user.click(mainTab(/Agents/));
    await user.click(await screen.findByRole("button", { name: "Import" }));
    await waitFor(() => expect(importMemory).toHaveBeenCalledTimes(1));
    expect(
      await screen.findByRole("heading", { name: "Imported Guru" }),
    ).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Rename" }));
    const name = screen.getByRole("textbox", { name: "Name" });
    await user.clear(name);
    await user.type(name, "Imported Analyst");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() =>
      expect(rename).toHaveBeenCalledWith({
        guru_id: expect.stringMatching(/^guru-imported/),
        name: "Imported Analyst",
      }),
    );
    expect(
      await screen.findByRole("heading", { name: "Imported Analyst" }),
    ).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Export" }));
    await waitFor(() => expect(exportMemory).toHaveBeenCalledTimes(1));

    await user.click(screen.getByRole("button", { name: "Delete" }));
    expect(
      screen.getByRole("heading", { name: "Delete agent?" }),
    ).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Delete agent" }));
    await waitFor(() => expect(remove).toHaveBeenCalledTimes(1));
    expect(
      await screen.findByRole("heading", { name: "Cycle Reader" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("heading", { name: "Delete agent?" }),
    ).not.toBeInTheDocument();
  });

  it("clears the Chat draft and loads the destination Chat memory policy", async () => {
    const user = userEvent.setup();
    await openApp();

    const prompt = screen.getByRole("textbox", { name: "Message Guru" });
    const updateMemory = screen.getByRole("checkbox", {
      name: "Update memory",
    });
    await user.type(prompt, "A draft that belongs only to the previous Guru");
    await user.click(updateMemory);

    await chooseGuru(user, "Contrarian Value");
    await screen.findByRole("heading", { name: "Downside scenario review" });

    expect(screen.getByRole("textbox", { name: "Message Guru" })).toHaveValue(
      "",
    );
    expect(
      screen.getByRole("checkbox", { name: "Update memory" }),
    ).toBeChecked();
  });

  it("ignores a late thread created for the previously selected Guru", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    let resolveCreate!: (thread: ChatThread) => void;
    vi.spyOn(bridge, "chatCreate").mockImplementation(
      () =>
        new Promise<ChatThread>((resolve) => {
          resolveCreate = resolve;
        }),
    );

    await user.click(
      screen.getByRole("button", {
        name: "New session for Quality Compounder",
      }),
    );
    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Late chat from the previous Guru",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(bridge.chatCreate).toHaveBeenCalled());
    await chooseGuru(user, "Contrarian Value");
    await screen.findByRole("heading", { name: "Downside scenario review" });

    await act(async () => {
      resolveCreate({
        id: "thread-from-previous-guru",
        guru_id: "guru-quality",
        title: "Late chat from the previous Guru",
        updated_at: "2026-08-08T00:00:00Z",
        use_memory: true,
        update_memory: true,
        messages: [],
      });
    });

    expect(
      screen.queryByRole("button", {
        name: /Late chat from the previous Guru/,
      }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "Downside scenario review" }),
    ).toBeVisible();
  });

  it("routes the first-run zero state to model provider settings", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const catalog = await bridge.modelCatalogGet();
    vi.spyOn(bridge, "modelCatalogGet").mockResolvedValue({
      hidden_model_profile_ids: [],
      models: catalog.models.map((model) => ({
        ...model,
        credential_source: "missing",
      })),
      providers: catalog.providers.map((provider) => ({
        ...provider,
        credential_source: "missing",
      })),
    });
    vi.spyOn(bridge, "guruList").mockResolvedValueOnce([]);
    render(<App bridge={bridge} />);

    const heading = await screen.findByRole("heading", {
      name: "Connect a model provider",
    });
    const onboarding = heading.closest("main");
    expect(onboarding).not.toBeNull();
    expect(
      within(onboarding!).queryByRole("button", {
        name: /One Memory per strategy/,
      }),
    ).not.toBeInTheDocument();
    expect(
      within(onboarding!).getByText(/You pay the provider/),
    ).toBeVisible();
    await user.click(
      within(onboarding!).getByRole("button", { name: "Open Settings" }),
    );
    expect(
      await screen.findByRole("heading", { name: "Settings" }),
    ).toBeVisible();
    expect(screen.getByText("Model providers")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Connect provider" }));
    expect(screen.getByRole("combobox", { name: "Provider" })).toBeVisible();
  });

  it("shows agent creation failures in the Agents zero state", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    vi.spyOn(bridge, "guruList").mockResolvedValueOnce([]);
    vi.spyOn(bridge, "guruCreate").mockRejectedValueOnce(
      new Error("Could not create this agent."),
    );
    render(<App bridge={bridge} />);

    await screen.findByRole("heading", { name: "Create a Guru" });
    await user.click(mainTab(/Agents/));
    await user.click(screen.getByRole("button", { name: /Create agent/ }));
    await user.type(screen.getByRole("textbox", { name: "Name" }), "My Guru");
    await user.click(screen.getByRole("button", { name: "Create agent" }));

    const dialog = screen.getByRole("dialog", { name: "Create agent" });
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "Could not create this agent.",
    );
    await user.click(within(dialog).getByRole("button", { name: "Close" }));
    const zeroState = screen
      .getByRole("heading", { name: "Create your first agent" })
      .closest("section");
    expect(zeroState).not.toBeNull();
    expect(within(zeroState!).getByRole("alert")).toHaveTextContent(
      "Could not create this agent.",
    );
  });

  it("does not offer a duplicate create retry after the agent was persisted", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const createGuru = vi.spyOn(bridge, "guruCreate");
    vi.spyOn(bridge, "guruList")
      .mockResolvedValueOnce([])
      .mockRejectedValueOnce(new Error("Refresh failed."));
    render(<App bridge={bridge} />);

    await screen.findByRole("heading", { name: "Create a Guru" });
    await user.click(mainTab(/Agents/));
    await user.click(screen.getByRole("button", { name: /Create agent/ }));
    await user.type(screen.getByRole("textbox", { name: "Name" }), "My Guru");
    await user.click(screen.getByRole("button", { name: "Create agent" }));

    await waitFor(() => {
      expect(
        screen.queryByRole("dialog", { name: "Create agent" }),
      ).not.toBeInTheDocument();
    });
    expect(createGuru).toHaveBeenCalledTimes(1);
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The agent was created, but the agent list could not be refreshed.",
    );
  });

  it("lets users close the create dialog while creation continues", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const createAgent = bridge.guruCreate.bind(bridge);
    let releaseCreate: () => void = () => {};
    const createPending = new Promise<void>((resolve) => {
      releaseCreate = () => resolve();
    });
    const createGuru = vi
      .spyOn(bridge, "guruCreate")
      .mockImplementation(async (request) => {
        await createPending;
        return createAgent(request);
      });
    vi.spyOn(bridge, "guruList").mockResolvedValueOnce([]);
    render(<App bridge={bridge} />);

    await screen.findByRole("heading", { name: "Create a Guru" });
    await user.click(mainTab(/Agents/));
    await user.click(screen.getByRole("button", { name: /Create agent/ }));
    await user.type(screen.getByRole("textbox", { name: "Name" }), "My Guru");
    await user.click(screen.getByRole("button", { name: "Create agent" }));

    const dialog = screen.getByRole("dialog", { name: "Create agent" });
    expect(
      within(dialog).getByRole("button", { name: "Creating…" }),
    ).toBeDisabled();
    await user.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(
      screen.queryByRole("dialog", { name: "Create agent" }),
    ).not.toBeInTheDocument();

    await act(async () => releaseCreate());
    expect(
      await screen.findByRole("heading", { name: "My Guru" }),
    ).toBeVisible();
    expect(createGuru).toHaveBeenCalledTimes(1);
  });
});
