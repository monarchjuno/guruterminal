import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MockGuruTerminalBridge } from "../../bridge";
import { openApp } from "../renderApp";

describe("Guru Terminal · Chat memory", () => {
  it("keeps composer memory options visible while a response is streaming", async () => {
    const user = userEvent.setup();
    await openApp(new MockGuruTerminalBridge({ delay_ms: 35 }));

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Keep the request controls stable while streaming",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));

    await screen.findByLabelText("Work progress");
    expect(screen.getByRole("checkbox", { name: "Use memory" })).toBeEnabled();
    expect(screen.getByRole("checkbox", { name: "Update memory" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Stop response" })).toBeVisible();

    await screen.findByText("Response complete.", {}, { timeout: 15_000 });
    expect(screen.getByRole("checkbox", { name: "Use memory" })).toBeEnabled();
  });

  it("keeps Use memory off for the request and renders no internal Memory trace", async () => {
    const user = userEvent.setup();
    await openApp(new MockGuruTerminalBridge({ delay_ms: 35 }));

    const useMemory = screen.getByRole("checkbox", { name: "Use memory" });
    expect(screen.getByRole("checkbox", { name: "Use memory" })).toBeChecked();
    await user.click(useMemory);
    expect(
      screen.getByRole("checkbox", { name: "Use memory" }),
    ).not.toBeChecked();

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Answer without a new framework",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));

    expect(
      await screen.findByText(/using only this conversation/),
    ).toBeVisible();
    expect(
      screen.queryByLabelText("Guru memory activity"),
    ).not.toBeInTheDocument();

    await user.click(
      screen.getByRole("button", { name: /Capital allocation checklist/ }),
    );
    expect(screen.getByRole("checkbox", { name: "Use memory" })).toBeChecked();
    await user.click(
      screen.getByRole("button", {
        name: /How should we read the margin decline/,
      }),
    );
    expect(screen.getByRole("checkbox", { name: "Use memory" })).not.toBeChecked();
    await screen.findByText("Response complete.", {}, { timeout: 15_000 });
    expect(
      screen.getByRole("checkbox", { name: "Use memory" }),
    ).not.toBeChecked();
  });

  it("keeps Memory provenance stored without exposing trace metadata in Chat", async () => {
    const bridge = new MockGuruTerminalBridge();
    const workspace = await bridge.guruSelect("guru-quality");
    workspace.threads[0].messages[0].memory_refs = [
      {
        record_id: "lens:quality",
        kind: "Lens",
        title: "INTERNAL_MEMORY_TITLE",
        excerpt: "INTERNAL_MEMORY_EXCERPT",
        as_of: "2026-08-06T00:00:00Z",
        access: "exact_read",
      },
    ];
    vi.spyOn(bridge, "guruSelect").mockResolvedValue(workspace);

    await openApp(bridge);

    expect(workspace.threads[0].messages[0].memory_refs).toHaveLength(1);
    expect(
      screen.queryByLabelText("Guru memory activity"),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("Opened")).not.toBeInTheDocument();
    expect(screen.queryByText("Found in search")).not.toBeInTheDocument();
    expect(screen.queryByText("INTERNAL_MEMORY_EXCERPT")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Used in this answer")).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Used note: INTERNAL_MEMORY_TITLE" }),
    ).toBeVisible();
  });

  it("keeps Update memory on and shows the automatic update result", async () => {
    const user = userEvent.setup();
    await openApp();

    const updateMemory = screen.getByRole("checkbox", {
      name: "Update memory",
    });
    expect(updateMemory).toBeChecked();

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Learn from this mistake",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));

    expect(updateMemory).toBeChecked();
    await user.click(await screen.findByText("Guru learned"));
    expect(
      screen.getByText("Separate recurring earnings quality from one-time margin movement."),
    ).toBeVisible();
    expect(screen.getByText("Basis: Current Chat evidence")).toBeVisible();
    expect(
      screen.getByText("Next use: This will change the checks used in later earnings research."),
    ).toBeVisible();
    await user.click(
      screen.getByRole("button", { name: "Earnings quality review" }),
    );

    const workspace = await screen.findByRole("complementary", {
      name: "Chat workspace panel",
    });
    expect(within(workspace).getByLabelText("Memory preview")).toBeVisible();
    expect(
      within(workspace).queryByLabelText("Document frontmatter"),
    ).not.toBeInTheDocument();
    expect(
      within(workspace).queryByRole("button", { name: "Open in Memory" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Message Guru" })).toBeVisible();
  });

  it("reverts an applied Chat memory write through the bridge", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const revertMemory = vi.spyOn(bridge, "libraryMemoryRevert");
    await openApp(bridge);

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Learn from this mistake",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));
    await user.click(await screen.findByText("Guru learned"));
    await user.click(screen.getByRole("button", { name: "Revert" }));

    expect(revertMemory).toHaveBeenCalledWith(
      expect.objectContaining({
        record_id: "lens:quality/earnings-quality",
        commit_id: expect.any(String),
      }),
    );
  });

  it("opens learned Memory from Chat without a revision history surface", async () => {
    const user = userEvent.setup();
    await openApp(new MockGuruTerminalBridge({ delay_ms: 0 }));

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Record this review and show me the exact change",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));
    await user.click(await screen.findByText("Guru learned"));
    expect(screen.getByRole("button", { name: "Earnings quality review" })).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "View changes" }),
    ).not.toBeInTheDocument();
  });

  it("labels a Decision-only update as a saved judgment", async () => {
    const bridge = new MockGuruTerminalBridge();
    const workspace = await bridge.guruSelect("guru-quality");
    workspace.threads[0].messages[0].memory_update = {
      status: "applied",
      commitId: "receipt-decision",
      changes: [
        {
          recordId: "decision:quality-call",
          kind: "Decision",
          operation: "create",
          title: "Hold the quality call",
          lesson: "Judgment sealed.",
          basis: "Current Chat evidence",
          futureUse: "Later outcome review.",
        },
      ],
    };
    vi.spyOn(bridge, "guruSelect").mockResolvedValue(workspace);
    await openApp(bridge);

    expect(await screen.findByText("Judgment saved")).toBeVisible();
    expect(screen.queryByText("Guru learned")).not.toBeInTheDocument();
  });

  it("treats research-only Wiki as Guru learned without a Decision", async () => {
    const bridge = new MockGuruTerminalBridge();
    const workspace = await bridge.guruSelect("guru-quality");
    workspace.threads[0].messages[0].memory_update = {
      status: "applied",
      commitId: "receipt-research-wiki",
      changes: [
        {
          recordId: "wiki:ev-industry",
          kind: "Wiki",
          operation: "create",
          title: "EV industry notes",
          lesson: "Compiled from current research.",
          basis: "Current Chat evidence",
          futureUse: "Later EV research starts from this page.",
        },
      ],
    };
    vi.spyOn(bridge, "guruSelect").mockResolvedValue(workspace);
    await openApp(bridge);

    await userEvent.setup().click(await screen.findByText("Guru learned"));
    expect(screen.getByRole("button", { name: "Open in Memory" })).toBeVisible();
    expect(screen.queryByText("No durable lesson")).not.toBeInTheDocument();
  });

  it("labels provenance-only persistence as saved Evidence", async () => {
    const bridge = new MockGuruTerminalBridge();
    const workspace = await bridge.guruSelect("guru-quality");
    workspace.threads[0].messages[0].memory_update = {
      status: "applied",
      commitId: "receipt-evidence",
      changes: [
        {
          recordId: "evidence:source-check",
          kind: "Evidence",
          operation: "create",
          title: "Source check",
          lesson: "Exact source captured.",
          basis: "Current Chat evidence",
          futureUse: "Available for a later exact read.",
        },
      ],
    };
    vi.spyOn(bridge, "guruSelect").mockResolvedValue(workspace);
    await openApp(bridge);

    await userEvent.setup().click(await screen.findByText("Sources saved"));
    const sourceLink = screen.getByRole("button", { name: "Source check" });
    expect(sourceLink).toBeVisible();
    expect(sourceLink.closest("li")).toBeTruthy();
    expect(
      screen.queryByRole("button", { name: "View changes" }),
    ).not.toBeInTheDocument();
  });

  it("shows an intentional no-learning result without treating it as failure", async () => {
    const bridge = new MockGuruTerminalBridge();
    const workspace = await bridge.guruSelect("guru-quality");
    workspace.threads[0].messages[0].memory_update = {
      status: "no_change",
      commitId: null,
      changes: [],
    };
    vi.spyOn(bridge, "guruSelect").mockResolvedValue(workspace);
    await openApp(bridge);

    expect(await screen.findByText("No durable lesson")).toBeVisible();
    expect(screen.queryByText("Learning failed")).not.toBeInTheDocument();
  });

  it("stores a sealed decision without repeating its internal payload in Chat", async () => {
    const bridge = new MockGuruTerminalBridge();
    const workspace = await bridge.guruSelect("guru-quality");
    workspace.threads[0].messages[0].decision = {
      payload: {
        stance: "neutral",
        horizon: "internal-horizon",
        probability: 0.62,
        thesis: "INTERNAL_DECISION_THESIS",
        evidence_ids: ["evidence:chat/internal"],
        risks: ["INTERNAL_DECISION_RISK"],
        invalidation_conditions: ["INTERNAL_DECISION_INVALIDATION"],
      },
      digest: "internal-decision-digest",
      sealed_at_ms: 1,
    };
    vi.spyOn(bridge, "guruSelect").mockResolvedValue(workspace);

    await openApp(bridge);

    expect(
      screen.queryByLabelText("Sealed Chat decision"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("INTERNAL_DECISION_THESIS"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("internal-decision-digest"),
    ).not.toBeInTheDocument();
  });

});
