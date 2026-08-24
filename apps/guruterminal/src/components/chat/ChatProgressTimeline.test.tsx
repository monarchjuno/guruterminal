import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "react";
import { ChatProgressTimeline } from "./ChatProgressTimeline";

const runningProgress: ComponentProps<typeof ChatProgressTimeline>["progress"] =
  {
    startedAtMs: Date.now() - 4_000,
    items: [
      {
        id: "web-1",
        kind: "tool",
        category: "web",
        operation: "search",
        action: "Searched the web",
        target: "rate outlook",
        status: "succeeded",
        startedAtMs: Date.now() - 4_000,
        finishedAtMs: Date.now() - 3_000,
      },
      {
        id: "web-2",
        kind: "tool",
        category: "web",
        operation: "read",
        action: "Read a web source",
        target: "Rate outlook · example.com",
        href: "https://example.com/rates",
        status: "running",
        startedAtMs: Date.now() - 2_000,
      },
      {
        id: "commentary-3",
        kind: "commentary",
        text: "I will compare this with Memory.",
      },
      {
        id: "memory-4",
        kind: "tool",
        category: "memory",
        operation: "read",
        action: "Read Memory",
        target: "lens:rates",
        status: "succeeded",
        startedAtMs: Date.now() - 1_500,
        finishedAtMs: Date.now() - 1_000,
      },
      {
        id: "web-5",
        kind: "tool",
        category: "web",
        operation: "search",
        action: "Searched the web again",
        status: "succeeded",
        startedAtMs: Date.now() - 800,
        finishedAtMs: Date.now() - 200,
      },
    ],
  };

describe("ChatProgressTimeline", () => {
  it("uses a plain, line-preserving projection for actively changing commentary", () => {
    const first = {
      startedAtMs: Date.now(),
      items: [
        {
          id: "commentary-1",
          kind: "commentary" as const,
          text: "1. First streamed line",
        },
      ],
    };
    const { container, rerender } = render(
      <ChatProgressTimeline
        progress={first}
        status="streaming"
        onOpenLink={async () => undefined}
      />,
    );

    const live = container.querySelector(".chat-progress-commentary-live");
    expect(live).toHaveTextContent("1. First streamed line");
    expect(live?.textContent).toBe("1. First streamed line");

    rerender(
      <ChatProgressTimeline
        progress={{
          ...first,
          items: [
            {
              id: "commentary-1",
              kind: "commentary",
              text: "1. First streamed line\n2. Second streamed line",
            },
          ],
        }}
        status="streaming"
        onOpenLink={async () => undefined}
      />,
    );

    expect(
      container.querySelector(".chat-progress-commentary-live")?.textContent,
    ).toBe("1. First streamed line\n2. Second streamed line");
  });

  it("keeps a live group stable as actions settle without outcome glyphs", async () => {
    const firstAction = runningProgress.items[0]!;
    const secondAction = runningProgress.items[1]!;
    if (
      firstAction.kind === "commentary" ||
      secondAction.kind === "commentary"
    ) {
      throw new Error("fixture actions are malformed");
    }
    const user = userEvent.setup();
    const { rerender } = render(
      <ChatProgressTimeline
        progress={{
          startedAtMs: Date.now(),
          items: [firstAction],
        }}
        status="streaming"
        onOpenLink={async () => undefined}
      />,
    );

    expect(screen.getByText("Searched the web")).toBeVisible();
    expect(screen.queryByLabelText("Done")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Failed")).not.toBeInTheDocument();

    rerender(
      <ChatProgressTimeline
        progress={{
          startedAtMs: Date.now(),
          items: [firstAction, secondAction],
        }}
        status="streaming"
        onOpenLink={async () => undefined}
      />,
    );

    const groupToggle = screen.getByRole("button", {
      name: /Web research · 2 actions/,
    });
    expect(groupToggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("Searched the web")).not.toBeInTheDocument();
    expect(screen.queryByText("Read a web source")).not.toBeInTheDocument();

    await user.click(groupToggle);
    expect(groupToggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("Searched the web")).toBeVisible();
    expect(screen.getByText("Read a web source")).toBeVisible();

    rerender(
      <ChatProgressTimeline
        progress={{
          startedAtMs: Date.now(),
          items: [firstAction, { ...secondAction, status: "failed" as const }],
        }}
        status="streaming"
        onOpenLink={async () => undefined}
      />,
    );

    expect(groupToggle).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByText("Searched the web")).toBeVisible();
    expect(screen.getByText("Read a web source")).toBeVisible();
    expect(screen.queryByLabelText("Done")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Failed")).not.toBeInTheDocument();

    rerender(
      <ChatProgressTimeline
        progress={{
          startedAtMs: Date.now(),
          finishedAtMs: Date.now(),
          items: [firstAction, { ...secondAction, status: "failed" as const }],
        }}
        status="error"
        onOpenLink={async () => undefined}
      />,
    );
    expect(
      screen.getByRole("button", { name: /Work failed · 2 steps/ }),
    ).toBeVisible();
    expect(screen.getByLabelText("Work progress")).toHaveClass("has-failure");
    expect(screen.queryByLabelText("Done")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Failed")).not.toBeInTheDocument();

    rerender(
      <ChatProgressTimeline
        progress={{
          startedAtMs: Date.now(),
          finishedAtMs: Date.now(),
          items: [firstAction, { ...secondAction, status: "failed" as const }],
        }}
        status="complete"
        onOpenLink={async () => undefined}
      />,
    );
    expect(
      screen.getByRole("button", { name: /2 steps · Web research · / }),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: /Work failed/ }),
    ).not.toBeInTheDocument();
    expect(screen.getByLabelText("Work progress")).not.toHaveClass(
      "has-failure",
    );
  });

  it("renders plain commentary and groups only consecutive matching actions", async () => {
    const user = userEvent.setup();
    const openLink = vi.fn(async () => undefined);
    const { container, rerender } = render(
      <ChatProgressTimeline
        progress={runningProgress}
        status="streaming"
        onOpenLink={openLink}
      />,
    );

    const timeline = screen.getByLabelText("Work progress");
    const toggle = within(timeline).getByRole("button", {
      name: /Working · Read a web source · \d+s/,
    });
    expect(toggle).toHaveAttribute("aria-expanded", "true");

    const groups = container.querySelectorAll(".chat-progress-group");
    expect(groups).toHaveLength(1);
    expect(groups[0]).toHaveAttribute("data-progress-category", "web");
    const webGroupToggle = within(groups[0] as HTMLElement).getByRole(
      "button",
      { name: /Web research · 2 actions/ },
    );
    expect(webGroupToggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText("Searched the web")).not.toBeInTheDocument();

    await user.click(webGroupToggle);
    expect(webGroupToggle).toHaveAttribute("aria-expanded", "true");

    const commentary = await within(timeline).findByText(
      "I will compare this with Memory.",
    );
    const commentaryRow = commentary.closest(".chat-progress-commentary");
    expect(commentaryRow).toBeVisible();
    expect(commentaryRow?.querySelector("svg")).toBeNull();

    const memory = within(timeline).getByText("Read Memory");
    const finalWeb = within(timeline).getByText("Searched the web again");
    expect(memory.closest(".chat-progress-group")).toBeNull();
    expect(finalWeb.closest(".chat-progress-group")).toBeNull();
    expect(
      webGroupToggle.compareDocumentPosition(commentary) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(
      commentary.compareDocumentPosition(memory) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();

    await user.click(
      within(timeline).getByRole("button", {
        name: "Rate outlook · example.com",
      }),
    );
    expect(openLink).toHaveBeenCalledWith("https://example.com/rates");

    const parallel = {
      ...runningProgress,
      items: runningProgress.items.map((item) =>
        item.id === "memory-4" ? { ...item, status: "running" as const } : item,
      ),
    };
    rerender(
      <ChatProgressTimeline
        progress={parallel}
        status="streaming"
        onOpenLink={openLink}
      />,
    );
    expect(
      within(timeline).getByRole("button", { name: /2 actions running/ }),
    ).toBeVisible();
    expect(
      within(
        within(timeline)
          .getByText("Read Memory")
          .closest(".chat-progress-row")!,
      ).getByLabelText("Running"),
    ).toBeVisible();

    const completed = {
      ...runningProgress,
      finishedAtMs: Date.now(),
      items: runningProgress.items.map((item) =>
        item.kind === "tool" ? { ...item, status: "succeeded" as const } : item,
      ),
    };
    rerender(
      <ChatProgressTimeline
        progress={completed}
        status="complete"
        onOpenLink={openLink}
      />,
    );
    const completedToggle = within(timeline).getByRole("button", {
      name: /4 steps · Web research, Memory/,
    });
    expect(completedToggle).toHaveAttribute("aria-expanded", "false");

    await user.click(completedToggle);
    expect(
      within(timeline).getByText("I will compare this with Memory."),
    ).toBeVisible();
    expect(within(timeline).getByText("Read Memory")).toBeVisible();
    expect(within(timeline).queryByText("Searched the web")).toBeNull();

    const settledGroupToggle = within(timeline).getByRole("button", {
      name: /Web research · 2 actions/,
    });
    expect(settledGroupToggle).toHaveAttribute("aria-expanded", "false");
    await user.click(settledGroupToggle);
    expect(within(timeline).getByText("Searched the web")).toBeVisible();
    expect(within(timeline).getByText("Read a web source")).toBeVisible();
  });

  it("renders every commentary in timeline order between actions", () => {
    render(
      <ChatProgressTimeline
        progress={{
          startedAtMs: Date.now(),
          items: [
            {
              id: "commentary-1",
              kind: "commentary",
              text: "I will search first.",
            },
            {
              id: "web-1",
              kind: "tool",
              category: "web",
              operation: "search",
              action: "Searched the web",
              status: "succeeded",
              startedAtMs: Date.now() - 2_000,
              finishedAtMs: Date.now() - 1_000,
            },
            {
              id: "commentary-2",
              kind: "commentary",
              text: "The source is enough to continue.",
            },
            {
              id: "memory-1",
              kind: "tool",
              category: "memory",
              operation: "read",
              action: "Read Memory",
              status: "succeeded",
              startedAtMs: Date.now() - 800,
              finishedAtMs: Date.now() - 200,
            },
          ],
        }}
        status="streaming"
        onOpenLink={async () => undefined}
      />,
    );

    const first = screen.getByText("I will search first.");
    const search = screen.getByText("Searched the web");
    const second = screen.getByText("The source is enough to continue.");
    const memory = screen.getByText("Read Memory");
    expect(first.closest(".chat-progress-commentary")).toBeVisible();
    expect(second.closest(".chat-progress-commentary")).toBeVisible();
    expect(
      first.compareDocumentPosition(search) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(
      search.compareDocumentPosition(second) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(
      second.compareDocumentPosition(memory) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("exposes compact system rows with operation and status attributes", async () => {
    const user = userEvent.setup();
    const compactItem = (status: "succeeded" | "failed") => ({
      id: "compaction",
      kind: "system" as const,
      category: "system" as const,
      operation: "compact" as const,
      action: "Compacting conversation context",
      status,
      startedAtMs: Date.now() - 2_000,
      finishedAtMs: Date.now() - 500,
    });

    render(
      <ChatProgressTimeline
        progress={{
          startedAtMs: Date.now() - 2_000,
          finishedAtMs: Date.now(),
          items: [compactItem("succeeded")],
        }}
        status="complete"
        onOpenLink={async () => undefined}
      />,
    );

    const timeline = screen.getByLabelText("Work progress");
    await user.click(within(timeline).getByRole("button"));
    const succeeded = within(timeline)
      .getByText("Compacting conversation context")
      .closest(".chat-progress-row");
    expect(succeeded).toHaveAttribute("data-progress-category", "system");
    expect(succeeded).toHaveAttribute("data-progress-operation", "compact");
    expect(succeeded).toHaveAttribute("data-progress-status", "succeeded");
  });

  it("marks a failed compact row in the shipped progress attributes", async () => {
    const user = userEvent.setup();
    render(
      <ChatProgressTimeline
        progress={{
          startedAtMs: Date.now() - 2_000,
          finishedAtMs: Date.now(),
          items: [
            {
              id: "compaction",
              kind: "system",
              category: "system",
              operation: "compact",
              action: "Compacting conversation context",
              status: "failed",
              startedAtMs: Date.now() - 2_000,
              finishedAtMs: Date.now() - 500,
            },
          ],
        }}
        status="error"
        onOpenLink={async () => undefined}
      />,
    );

    const timeline = screen.getByLabelText("Work progress");
    await user.click(within(timeline).getByRole("button", { name: /Work failed/ }));
    const failed = within(timeline)
      .getByText("Compacting conversation context")
      .closest(".chat-progress-row");
    expect(failed).toHaveAttribute("data-progress-operation", "compact");
    expect(failed).toHaveAttribute("data-progress-status", "failed");
  });
});
