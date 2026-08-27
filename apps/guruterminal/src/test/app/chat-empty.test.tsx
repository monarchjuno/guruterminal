import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MockGuruTerminalBridge } from "../../bridge";
import { openApp } from "../renderApp";

describe("Guru Terminal · empty Chat", () => {
  it("opens a draft without adding a session until the first message", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const create = vi.spyOn(bridge, "chatCreate");

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
      screen.queryByRole("button", { name: "Set investment charter" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("textbox", { name: "SEC contact email" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Use $research" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Use $wiki" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Use $lens" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Use $decision" })).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "New chat" }),
    ).not.toBeInTheDocument();
    expect(create).not.toHaveBeenCalled();

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

    await user.type(
      screen.getByRole("textbox", { name: "Message Guru" }),
      "Samsung earnings",
    );
    await user.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() =>
      expect(create).toHaveBeenCalledWith({ guru_id: "guru-quality" }),
    );
    expect(
      await screen.findByRole("button", { name: /Samsung earnings/ }),
    ).toBeVisible();
  });

  it("routes a Lens skill chip into Chat with memory locked on", async () => {
    const user = userEvent.setup();
    await openApp();

    await user.click(
      screen.getByRole("button", {
        name: "New session for Quality Compounder",
      }),
    );
    await screen.findByRole("heading", { name: "New chat" });
    await user.click(screen.getByRole("button", { name: "Use $lens" }));

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
});
