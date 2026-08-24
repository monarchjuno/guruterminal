import { act, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { vi } from "vitest";

const mermaid = vi.hoisted(() => ({
  load: vi.fn(),
}));

vi.mock("./mermaidPlugin", () => ({
  hasCompleteMermaidFence: (text: string) => text === "closed-diagram",
  loadMermaidPlugin: mermaid.load,
}));

vi.mock("@/components/ai-elements/message-response", () => ({
  MessageResponse: ({
    children,
    mermaidPlugin,
  }: {
    children: ReactNode;
    mermaidPlugin?: unknown;
  }) => (
    <div data-mermaid={mermaidPlugin ? "loaded" : "base"} data-testid="response">
      {children}
    </div>
  ),
}));

import { MessageRenderBoundary, SafeMessageResponse } from "./SafeMessageResponse";

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

describe("SafeMessageResponse Mermaid loading", () => {
  beforeEach(() => {
    mermaid.load.mockReset();
  });

  function response(text: string, isAnimating = false) {
    return (
      <SafeMessageResponse
        isAnimating={isAnimating}
        onOpenLink={() => undefined}
        text={text}
      />
    );
  }

  it("keeps ordinary Markdown on the base renderer without loading Mermaid", async () => {
    render(response("ordinary Markdown"));

    expect(await screen.findByTestId("response")).toHaveAttribute(
      "data-mermaid",
      "base",
    );
    expect(mermaid.load).not.toHaveBeenCalled();
  });

  it("does not load a completed diagram until streaming has stopped", async () => {
    mermaid.load.mockResolvedValue({ name: "mermaid" });
    const rendered = render(response("closed-diagram", true));

    expect(await screen.findByTestId("response")).toHaveAttribute(
      "data-mermaid",
      "base",
    );
    expect(mermaid.load).not.toHaveBeenCalled();

    rendered.rerender(response("closed-diagram"));
    await waitFor(() => expect(mermaid.load).toHaveBeenCalledOnce());
  });

  it("upgrades only the completed diagram after the deferred plugin resolves", async () => {
    let resolvePlugin: ((plugin: { name: string }) => void) | undefined;
    mermaid.load.mockReturnValue(
      new Promise<{ name: string }>((resolve) => {
        resolvePlugin = resolve;
      }),
    );
    render(response("closed-diagram"));

    const rendered = await screen.findByTestId("response");
    await waitFor(() => expect(mermaid.load).toHaveBeenCalledOnce());
    expect(rendered).toHaveAttribute("data-mermaid", "base");

    await act(async () => {
      resolvePlugin?.({ name: "mermaid" });
    });
    await waitFor(() => expect(rendered).toHaveAttribute("data-mermaid", "loaded"));
  });

  it("keeps the base renderer when the deferred plugin cannot load", async () => {
    mermaid.load.mockRejectedValueOnce(new Error("unavailable"));
    render(response("closed-diagram"));

    const rendered = await screen.findByTestId("response");
    await waitFor(() => expect(mermaid.load).toHaveBeenCalledOnce());
    await waitFor(() => expect(rendered).toHaveAttribute("data-mermaid", "base"));
  });
});
