import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MockGuruTerminalBridge } from "../../bridge";
import { openApp } from "../renderApp";

describe("Guru Terminal · empty Chat", () => {
  it("inserts a Skill token from the empty state", async () => {
    const user = userEvent.setup();
    await openApp();

    await user.click(
      screen.getByRole("button", {
        name: "New session for Quality Compounder",
      }),
    );
    await screen.findByRole("heading", { name: "New chat" });

    expect(
      screen.getByRole("heading", {
        name: "Ask Quality Compounder",
      }),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Set investment charter" }),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "Use $research" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Use $wiki" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Use $lens" })).toBeVisible();
    expect(
      await screen.findByRole("textbox", { name: "SEC contact email" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Set up OpenDART" }),
    ).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Use $wiki" }));

    expect(screen.getByRole("textbox", { name: "Message Guru" })).toHaveValue(
      "$wiki ",
    );
    expect(screen.getByRole("checkbox", { name: "Use memory" })).toBeChecked();
    expect(
      screen.getByRole("checkbox", { name: "Update memory" }),
    ).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Use memory" })).toBeDisabled();
    expect(
      screen.getByRole("checkbox", { name: "Update memory" }),
    ).toBeDisabled();
  });

  it("routes the first-run teach affordance into Chat with $lens", async () => {
    const user = userEvent.setup();
    await openApp();

    await user.click(
      screen.getByRole("button", {
        name: "New session for Quality Compounder",
      }),
    );
    await screen.findByRole("heading", { name: "New chat" });
    await user.click(
      screen.getByRole("button", { name: "Set investment charter" }),
    );

    expect(screen.getByRole("textbox", { name: "Message Guru" })).toHaveValue(
      "$lens ",
    );
    expect(screen.getByRole("checkbox", { name: "Use memory" })).toBeChecked();
    expect(
      screen.getByRole("checkbox", { name: "Update memory" }),
    ).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Use memory" })).toBeDisabled();
    expect(
      screen.getByRole("checkbox", { name: "Update memory" }),
    ).toBeDisabled();
  });

  it("saves the EDGAR contact email from empty Chat without leaving Chat", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const configure = vi.spyOn(bridge, "marketplaceConnectorConfigure");
    const enable = vi.spyOn(bridge, "guruCapabilityEnable");
    await openApp(bridge);

    await user.click(
      screen.getByRole("button", {
        name: "New session for Quality Compounder",
      }),
    );
    await screen.findByRole("heading", { name: "New chat" });
    await user.type(
      await screen.findByRole("textbox", { name: "SEC contact email" }),
      "research@example.com",
    );
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(configure).toHaveBeenCalledWith({
        entry_id: "sec.edgar",
        config: { contact_email: "research@example.com" },
      }),
    );
    expect(enable).toHaveBeenCalledWith({
      guru_id: "guru-quality",
      entry_id: "sec.edgar",
    });
    expect(screen.getByRole("heading", { name: "New chat" })).toBeVisible();
    expect(
      screen.queryByRole("dialog", { name: "Set up SEC EDGAR" }),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Agents" })).not.toBeInTheDocument();
    await waitFor(() =>
      expect(
        screen.queryByRole("textbox", { name: "SEC contact email" }),
      ).not.toBeInTheDocument(),
    );
  });

  it("enables a configured EDGAR source from empty Chat without opening Agents", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    await bridge.marketplaceConnectorConfigure({
      entry_id: "sec.edgar",
      config: { contact_email: "research@example.com" },
    });
    const enable = vi.spyOn(bridge, "guruCapabilityEnable");
    await openApp(bridge);

    await user.click(
      screen.getByRole("button", {
        name: "New session for Quality Compounder",
      }),
    );
    await screen.findByRole("heading", { name: "New chat" });
    await user.click(
      await screen.findByRole("button", { name: "Enable SEC EDGAR" }),
    );

    await waitFor(() =>
      expect(enable).toHaveBeenCalledWith({
        guru_id: "guru-quality",
        entry_id: "sec.edgar",
      }),
    );
    expect(screen.getByRole("heading", { name: "New chat" })).toBeVisible();
    expect(screen.queryByRole("heading", { name: "Agents" })).not.toBeInTheDocument();
    await waitFor(() =>
      expect(
        screen.queryByRole("button", { name: "Enable SEC EDGAR" }),
      ).not.toBeInTheDocument(),
    );
  });
});
