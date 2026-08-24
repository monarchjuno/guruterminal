import { render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { MessageRenderBoundary } from "./SafeMessageResponse";

function BrokenResponse(): ReactNode {
  throw new Error("renderer unavailable");
}

describe("MessageRenderBoundary", () => {
  it("keeps response text visible when rich rendering fails", () => {
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined);

    try {
      render(
        <MessageRenderBoundary fallback={<p>Raw response text</p>}>
          <BrokenResponse />
        </MessageRenderBoundary>,
      );

      expect(screen.getByText("Raw response text")).toBeVisible();
    } finally {
      consoleError.mockRestore();
    }
  });
});
