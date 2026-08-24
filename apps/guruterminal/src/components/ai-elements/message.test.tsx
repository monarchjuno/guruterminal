import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MessageResponse } from "./message-response";

describe("MessageResponse links", () => {
  it("opens credential-free web links through the host callback", async () => {
    const user = userEvent.setup();
    const openLink = vi.fn().mockResolvedValue(undefined);
    const windowOpen = vi.spyOn(window, "open").mockImplementation(() => null);
    const url = "https://dart.fss.or.kr/dsab001/search.ax?textCrpNm=005930";

    render(
      <MessageResponse mode="static" onOpenLink={openLink}>
        {`[DART company search](${url})`}
      </MessageResponse>,
    );

    await user.click(screen.getByRole("link", { name: "DART company search" }));
    expect(openLink).toHaveBeenCalledOnce();
    expect(openLink).toHaveBeenCalledWith(url);
    expect(windowOpen).not.toHaveBeenCalled();
    windowOpen.mockRestore();
  });

  it("blocks unsupported and credential-bearing URLs before they reach the host", () => {
    const openLink = vi.fn();
    render(
      <MessageResponse mode="static" onOpenLink={openLink}>
        {[
          "[Local file](file:///tmp/private)",
          "[Script](javascript:alert(1))",
          "[Credential URL](https://user:secret@example.com/report)",
          "[Relative](/report)",
        ].join("\n\n")}
      </MessageResponse>,
    );

    expect(screen.queryByRole("link")).not.toBeInTheDocument();
    expect(screen.getAllByText("Unsupported link.").length).toBeGreaterThan(0);
    expect(openLink).not.toHaveBeenCalled();
  });

  it("announces a host open failure without creating an unhandled rejection", async () => {
    const user = userEvent.setup();
    const openLink = vi.fn().mockRejectedValue(new Error("open failed"));
    render(
      <MessageResponse mode="static" onOpenLink={openLink}>
        [Example](https://example.com/report)
      </MessageResponse>,
    );

    await user.click(screen.getByRole("link", { name: "Example" }));
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent(
        "Could not open this link.",
      ),
    );
  });
});
