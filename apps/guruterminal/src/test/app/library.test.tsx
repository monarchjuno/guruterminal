import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MockGuruTerminalBridge } from "../../bridge";
import { LibraryView } from "../../components/LibraryView";
import { mainTab, openApp } from "../renderApp";

const findListedMemory = (title: string) =>
  screen.findByRole("button", { name: new RegExp(`^Open ${title}\\b`) });

describe("Guru Terminal · Library", () => {
  it("explains a genuinely empty Library without presenting a failed search", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    vi.spyOn(bridge, "librarySearch").mockResolvedValue([]);
    const guru = { ...(await bridge.guruList())[0], record_count: 0 };
    const onTeachInChat = vi.fn();

    render(
      <LibraryView
        bridge={bridge}
        guru={guru}
        requestedMemory={null}
        onRequestConsumed={() => undefined}
        onTeachInChat={onTeachInChat}
        refreshToken={0}
      />,
    );

    expect(
      await screen.findByRole("heading", {
        name: "No memories yet",
      }),
    ).toBeVisible();
    expect(screen.queryByText("No matching memories")).toBeNull();
    expect(screen.queryByText("Select a memory")).toBeNull();
    expect(screen.getByText("0 results")).toBeVisible();
    expect(screen.getByText(/Wiki and Lens are learned state/)).toBeVisible();
    await user.click(
      screen.getByRole("button", { name: "Open Chat" }),
    );
    expect(onTeachInChat).toHaveBeenCalledOnce();
  });

  it("opens a page from the list and shows backlinks", async () => {
    const user = userEvent.setup();
    await openApp();
    await user.click(mainTab(/Memory/));
    expect(
      await screen.findByRole("heading", { name: "Overview" }),
    ).toBeVisible();
    expect(
      await screen.findByRole("heading", { name: "Learned state" }),
    ).toBeVisible();
    expect(
      screen.getByRole("heading", { name: "Learning input" }),
    ).toBeVisible();
    expect(
      await findListedMemory("Margin bridge example"),
    ).toBeVisible();

    const toolbar = screen.getByRole("toolbar", {
      name: "Filter memory by type",
    });
    await user.click(within(toolbar).getByRole("button", { name: "Evidence" }));
    expect(
      await screen.findByRole("heading", {
        name: "Margin bridge example",
        level: 1,
      }),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", {
        name: "Open Earnings quality review (Lens, Learned state)",
      }),
    ).not.toBeInTheDocument();

    await user.click(within(toolbar).getByRole("button", { name: "All types" }));
    await user.click(await findListedMemory("Earnings quality review"));
    expect(
      await screen.findByRole("heading", { name: "Backlinks" }),
    ).toBeVisible();
    expect(
      screen.getByRole("button", {
        name: "Open related memory: Defer the quality-impairment call",
      }),
    ).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Show all memories" }));
    expect(
      await screen.findByRole("heading", { name: "Overview" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("heading", { name: "Nearby pages", level: 2 }),
    ).not.toBeInTheDocument();
  });

  it("navigates Memory by exact authored relationship", async () => {
    const user = userEvent.setup();
    await openApp();
    await user.click(mainTab(/Memory/));

    expect(await screen.findByText("5 results")).toBeVisible();
    expect(
      await screen.findByRole("heading", { name: "Overview" }),
    ).toBeVisible();

    await user.click(await findListedMemory("Earnings quality review"));
    expect(
      await screen.findByRole("heading", {
        name: "Earnings quality review",
        level: 1,
      }),
    ).toBeVisible();
    expect(document.querySelector(".record-role")?.textContent).toBe(
      "Learned state",
    );
    expect(screen.getByText(/As of.*Aug 6/)).toBeVisible();
    expect(screen.queryByText("Read only")).toBeNull();
    expect(screen.getByRole("heading", { name: "Related" })).toBeVisible();
    expect(screen.getByRole("heading", { name: "Uses" })).toBeVisible();
    expect(screen.getByRole("heading", { name: "Supports" })).toBeVisible();
    await user.click(screen.getByText("More"));
    expect(screen.getByText("lens:quality/earnings-quality")).toBeVisible();
    await user.keyboard("{Escape}");

    await user.click(
      screen.getByRole("button", {
        name: "Open related memory: Durable moat lens",
      }),
    );
    expect(
      await screen.findByRole("heading", {
        name: "Durable moat lens",
        level: 1,
      }),
    ).toBeVisible();
    await user.click(screen.getByText("More"));
    expect(screen.getByText("lens:quality/durable-moat")).toBeVisible();
    await user.keyboard("{Escape}");
  });

  it("locates Memory with keyboard-first search", async () => {
    const user = userEvent.setup();
    await openApp();
    await user.click(mainTab(/Memory/));
    await screen.findByText("5 results");

    await user.keyboard("/");
    const search = screen.getByRole("textbox", { name: "Search memory" });
    expect(search).toHaveFocus();
    await user.type(search, "durable moat");

    const result = await within(
      screen.getByLabelText("Memory search results"),
    ).findByRole("button", {
      name: "Open Durable moat lens (Lens, Learned state)",
    });
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(result).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(
      await screen.findByRole("heading", {
        name: "Durable moat lens",
        level: 1,
      }),
    ).toBeVisible();

    await user.click(mainTab(/Chat/));
    await user.keyboard("/");
    expect(
      screen.getByRole("textbox", { name: "Search memory", hidden: true }),
    ).not.toHaveFocus();
  });

  it("filters Memory with kind chips instead of a type dropdown", async () => {
    const user = userEvent.setup();
    await openApp();
    await user.click(mainTab(/Memory/));
    await screen.findByText("5 results");

    const toolbar = screen.getByRole("toolbar", {
      name: "Filter memory by type",
    });
    expect(
      within(toolbar).getByRole("button", { name: "All types" }),
    ).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();

    await user.click(within(toolbar).getByRole("button", { name: "Wiki" }));
    expect(
      within(toolbar).getByRole("button", { name: "Wiki" }),
    ).toHaveAttribute("aria-pressed", "true");
  });

  it("treats Evidence as a sealed learning input", async () => {
    const user = userEvent.setup();
    await openApp();
    await user.click(mainTab(/Memory/));
    await user.click(await findListedMemory("Margin bridge example"));

    expect(
      await screen.findByRole("heading", {
        name: "Margin bridge example",
        level: 1,
      }),
    ).toBeVisible();
    expect(screen.getByText("Learning input · sealed")).toBeVisible();
    expect(
      screen.getByText("Chat can learn from this page without rewriting it."),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Edit" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete" })).toBeVisible();
  });

  it("returns to the overview after a no-match search is cleared", async () => {
    const user = userEvent.setup();
    await openApp(new MockGuruTerminalBridge({ delay_ms: 0 }));
    await user.click(mainTab(/Memory/));
    expect(
      await screen.findByRole("heading", { name: "Overview" }),
    ).toBeVisible();
    await user.click(await findListedMemory("Earnings quality review"));
    expect(
      await screen.findByRole("heading", {
        name: "Earnings quality review",
        level: 1,
      }),
    ).toBeVisible();

    const search = screen.getByRole("textbox", { name: "Search memory" });
    fireEvent.change(search, { target: { value: "no-such-memory" } });
    expect(await screen.findByText("No matching memories")).toBeVisible();
    search.focus();
    await user.keyboard("{Escape}");
    expect(search).toHaveValue("");
    await user.click(screen.getByRole("button", { name: "Show all memories" }));

    expect(
      await screen.findByRole("heading", { name: "Overview" }),
    ).toBeVisible();
    expect(screen.queryByText("Select a memory")).not.toBeInTheDocument();
    await user.click(await findListedMemory("Earnings quality review"));
    expect(
      await screen.findByRole("heading", {
        name: "Earnings quality review",
        level: 1,
      }),
    ).toBeVisible();
  });

  it("toggles a Library record between rendered and raw markdown", async () => {
    const user = userEvent.setup();
    await openApp();
    await user.click(mainTab(/Memory/));
    await user.click(await findListedMemory("Earnings quality review"));
    await screen.findByRole("heading", {
      name: "Earnings quality review",
      level: 1,
    });

    await user.click(screen.getByRole("button", { name: "Raw" }));
    expect(document.querySelector(".raw-markdown")?.textContent).toContain(
      "# Earnings quality review",
    );

    await user.click(mainTab(/Memory/));
    expect(document.querySelector(".raw-markdown")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Rendered" }));
    await screen.findByRole("heading", {
      name: "Earnings quality review",
      level: 1,
    });
    expect(screen.queryByRole("button", { name: "Ask in Chat" })).toBeNull();
  });

  it("hides source metadata and deep-links headings", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const originalRead = bridge.libraryRead.bind(bridge);
    vi.spyOn(bridge, "libraryRead").mockImplementation(
      async (guruId, recordId) => {
        const record = await originalRead(guruId, recordId);
        if (record.id !== "lens:quality/earnings-quality") return record;
        return {
          ...record,
          markdown: `---
id: lens:quality/earnings-quality
kind: Lens
status: active
---
# Earnings quality review

Break quarterly margin changes down by cause.

## Review sequence

- Changes in pricing and product mix

## Decision rule

Record both evidence and counterexamples.`,
        };
      },
    );
    await openApp(bridge);
    await user.click(mainTab(/Memory/));
    await user.click(await findListedMemory("Earnings quality review"));
    await screen.findByRole("heading", {
      name: "Earnings quality review",
      level: 1,
    });

    expect(
      screen.queryByLabelText("Document frontmatter"),
    ).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Document details")).not.toBeInTheDocument();
    expect(
      within(screen.getByRole("article")).queryByText(
        "lens:quality/earnings-quality",
      ),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: "Review sequence" }),
    ).toHaveAttribute(
      "href",
      "#memory-lens-quality-earnings-quality-review-sequence",
    );
    expect(screen.queryByRole("button", { name: "Ask in Chat" })).toBeNull();
    expect(
      screen.queryByRole("toolbar", { name: "Actions for selected text" }),
    ).toBeNull();
  });

  it("does not offer a New Wiki or Lens or Graph surface", async () => {
    await openApp(new MockGuruTerminalBridge({ delay_ms: 0 }));
    await userEvent.setup().click(mainTab(/Memory/));
    await screen.findByText("5 results");

    expect(
      screen.queryByRole("button", { name: "New Wiki or Lens" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Graph" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Memory view")).not.toBeInTheDocument();
    expect(
      await screen.findByRole("heading", { name: "Overview" }),
    ).toBeVisible();
  });

  it("edits a Memory page under its title, not the record id", async () => {
    const user = userEvent.setup();
    await openApp(new MockGuruTerminalBridge({ delay_ms: 0 }));
    await user.click(mainTab(/Memory/));
    await user.click(await findListedMemory("Earnings quality review"));
    await screen.findByRole("heading", {
      name: "Earnings quality review",
      level: 1,
    });
    await user.click(screen.getByRole("button", { name: "Edit" }));

    expect(
      screen.getByRole("heading", { name: "Edit Earnings quality review" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("heading", {
        name: "Edit lens:quality/earnings-quality",
      }),
    ).not.toBeInTheDocument();
  });

  it("uses loaded Memory pages when the Guru summary still says zero", async () => {
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const guru = { ...(await bridge.guruList())[0], record_count: 0 };

    render(
      <LibraryView
        bridge={bridge}
        guru={guru}
        requestedMemory={null}
        onRequestConsumed={() => undefined}
        onTeachInChat={() => undefined}
        refreshToken={0}
      />,
    );

    expect(
      await screen.findByText(/5 pages\./),
    ).toBeVisible();
    expect(
      screen.queryByRole("heading", { name: "No memories yet" }),
    ).not.toBeInTheDocument();
    expect(
      await screen.findByRole("button", {
        name: "Open Earnings quality review (Lens, Learned state)",
      }),
    ).toBeVisible();
  });

  it("reverts a selected learned Memory record", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const revertMemory = vi.spyOn(bridge, "libraryMemoryRevert").mockResolvedValue({
      commit_id: "commit-revert",
      record_id: "lens:quality/earnings-quality",
    });
    await openApp(bridge);
    await user.click(mainTab(/Memory/));
    await user.click(await findListedMemory("Earnings quality review"));

    await user.click(screen.getByRole("button", { name: "Revert" }));

    expect(revertMemory).toHaveBeenCalledWith(
      expect.objectContaining({
        record_id: "lens:quality/earnings-quality",
        expected_markdown: expect.any(String),
      }),
    );
  });

  it("deletes a selected Memory record", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const deleteMemory = vi.spyOn(bridge, "libraryMemoryDelete");
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    await openApp(bridge);
    await user.click(mainTab(/Memory/));
    await user.click(await findListedMemory("Margin bridge example"));

    await user.click(screen.getByRole("button", { name: "Delete" }));

    expect(confirm).toHaveBeenCalledWith('Delete “Margin bridge example”?');
    expect(deleteMemory).toHaveBeenCalledWith(
      expect.objectContaining({
        record_id: "evidence:sample/margin-bridge",
      }),
    );
    expect(
      screen.queryByRole("heading", { name: "Margin bridge example" }),
    ).not.toBeInTheDocument();
  });

  it("filters unused pages without treating them as a failed search", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const originalSearch = bridge.librarySearch.bind(bridge);
    vi.spyOn(bridge, "librarySearch").mockImplementation(async (request) => {
      const records = await originalSearch(request);
      return records.map((record) =>
        record.id === "wiki:quality/roic"
          ? { ...record, status: "revoked" as const }
          : record,
      );
    });
    await openApp(bridge);
    await user.click(mainTab(/Memory/));
    expect(await screen.findByText("5 results")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Unused" }));

    expect(
      await findListedMemory("Principles for interpreting ROIC"),
    ).toBeVisible();
    expect(
      within(screen.getByLabelText("Memory search results")).getByText(
        "Unused",
      ),
    ).toBeVisible();
    expect(screen.queryByText("No matching memories")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", {
        name: "Open Earnings quality review (Lens, Learned state)",
      }),
    ).not.toBeInTheDocument();
  });

  it("surfaces a search failure without calling it an empty Library", async () => {
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    vi.spyOn(bridge, "librarySearch").mockRejectedValue(
      new Error("Memory sidecar unavailable"),
    );
    const guru = (await bridge.guruList())[0];

    render(
      <LibraryView
        bridge={bridge}
        guru={guru}
        requestedMemory={null}
        onRequestConsumed={() => undefined}
        onTeachInChat={() => undefined}
        refreshToken={0}
      />,
    );

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Memory sidecar unavailable",
    );
    expect(screen.getByRole("button", { name: "Try again" })).toBeVisible();
    expect(
      screen.queryByRole("heading", { name: "No memories yet" }),
    ).not.toBeInTheDocument();
  });

  it("keeps the overview copy distinct from a failed search", async () => {
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const guru = (await bridge.guruList())[0];
    render(
      <LibraryView
        bridge={bridge}
        guru={guru}
        requestedMemory={null}
        onRequestConsumed={() => undefined}
        onTeachInChat={() => undefined}
        refreshToken={0}
      />,
    );
    expect(await screen.findByText("5 results")).toBeVisible();
    const overview = (
      await screen.findByRole("heading", { name: "Overview" })
    ).closest(".library-home");
    expect(overview).not.toBeNull();
    expect(
      within(overview as HTMLElement).getByText(
        /Evidence and Decision are learning inputs/,
      ),
    ).toBeVisible();
    expect(screen.queryByText("No matching memories")).not.toBeInTheDocument();
  });
});
