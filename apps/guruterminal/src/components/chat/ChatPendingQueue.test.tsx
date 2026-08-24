import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ChatPendingQueue } from "./ChatPendingQueue";

describe("ChatPendingQueue", () => {
  it("edits, reorders, sends, and removes queued messages", async () => {
    const user = userEvent.setup();
    const onRemove = vi.fn();
    const onEdit = vi.fn();
    const onMove = vi.fn();
    const onSendNow = vi.fn();

    render(
      <ChatPendingQueue
        queued={[
          { id: "q1", text: "First queued", createdAt: "2026-08-17T00:00:00Z" },
          { id: "q2", text: "Second queued", createdAt: "2026-08-17T00:00:01Z" },
        ]}
        holdReason="Response stopped. Queued messages were kept."
        onRemove={onRemove}
        onEdit={onEdit}
        onMove={onMove}
        onSendNow={onSendNow}
        canSendNow
      />,
    );

    expect(
      screen.getByText("Response stopped. Queued messages were kept."),
    ).toBeVisible();

    await user.click(
      screen.getAllByRole("button", { name: "Edit queued message" })[0]!,
    );
    await user.clear(screen.getByLabelText("Queued message text"));
    await user.type(screen.getByLabelText("Queued message text"), "Edited first");
    await user.click(screen.getByRole("button", { name: "Save queued message" }));
    expect(onEdit).toHaveBeenCalledWith("q1", "Edited first");

    await user.click(
      screen.getAllByRole("button", { name: "Move queued message down" })[0]!,
    );
    expect(onMove).toHaveBeenCalledWith("q1", 1);

    await user.click(
      screen.getAllByRole("button", { name: "Send queued message now" })[0]!,
    );
    expect(onSendNow).toHaveBeenCalledWith("q1");

    await user.click(
      screen.getAllByRole("button", { name: "Remove queued message" })[0]!,
    );
    expect(onRemove).toHaveBeenCalledWith("q1");
  });

  it("does not send a queued message while the current response is running", async () => {
    const user = userEvent.setup();
    const onSendNow = vi.fn();

    render(
      <ChatPendingQueue
        queued={[
          { id: "q1", text: "Wait for the answer", createdAt: "2026-08-17T00:00:00Z" },
        ]}
        onRemove={vi.fn()}
        onEdit={vi.fn()}
        onMove={vi.fn()}
        onSendNow={onSendNow}
        canSendNow={false}
      />,
    );

    expect(
      screen.getByRole("button", { name: "Send queued message now" }),
    ).toBeDisabled();
    await user.click(
      screen.getByRole("button", { name: "Send queued message now" }),
    );
    expect(onSendNow).not.toHaveBeenCalled();
  });
});
