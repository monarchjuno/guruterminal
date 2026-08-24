import { act, render, screen } from "@testing-library/react";
import { useIsMobile } from "../hooks/use-mobile";

function MobileProbe() {
  return <output>{useIsMobile() ? "mobile" : "desktop"}</output>;
}

it("updates the responsive shell when the window crosses the mobile breakpoint", async () => {
  const originalWidth = window.innerWidth;
  Object.defineProperty(window, "innerWidth", {
    configurable: true,
    value: 700,
  });
  const view = render(<MobileProbe />);
  try {
    expect(await screen.findByText("mobile")).toBeVisible();
    Object.defineProperty(window, "innerWidth", {
      configurable: true,
      value: 1280,
    });
    act(() => window.dispatchEvent(new Event("resize")));
    expect(await screen.findByText("desktop")).toBeVisible();
  } finally {
    view.unmount();
    Object.defineProperty(window, "innerWidth", {
      configurable: true,
      value: originalWidth,
    });
  }
});
