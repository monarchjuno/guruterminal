import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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
      screen.queryByRole("button", { name: "Set investment charter" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("textbox", { name: "SEC contact email" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Use $research" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Use $wiki" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Use $lens" })).toBeVisible();
    expect(screen.getByRole("button", { name: "Use $decision" })).toBeVisible();

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
