import {
  render,
  screen,
  within,
} from "@testing-library/react";
import { App } from "../App";
import { MockGuruTerminalBridge } from "../bridge";

export const openApp = async (
  bridge = new MockGuruTerminalBridge({ delay_ms: 0 }),
) => {
  render(<App bridge={bridge} />);
  await screen.findByRole("heading", {
    name: "How should we read the margin decline?",
  });
  await screen.findByText(
    /Want to determine whether this quarter's margin decline is temporary/,
  );
  return bridge;
};

export const mainTab = (name: RegExp) =>
  within(screen.getByRole("navigation", { name: "Main views" })).getByRole(
    "button",
    { name },
  );

export const chooseGuru = async (
  user: { click: (element: Element) => Promise<unknown> },
  name: string,
) => {
  await user.click(screen.getByRole("button", { name }));
};
