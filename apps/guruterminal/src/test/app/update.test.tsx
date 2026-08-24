import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MockGuruTerminalBridge } from "../../bridge";
import type { UpdateState } from "../../types";
import { openApp } from "../renderApp";

const OFFER_ID = "d7fb1bc8-0678-42f5-a20c-2ec5dc953728";

const currentRelease: UpdateState = {
  supported: true,
  current_version: "0.0.1",
  phase: "idle",
  offer: null,
  downloaded_bytes: 0,
  total_bytes: null,
  last_checked_at_ms: 1_786_233_600_000,
  next_auto_check_at_ms: 1_786_320_000_000,
  error: null,
  blockers: [],
};

const availableRelease: UpdateState = {
  ...currentRelease,
  offer: {
    offer_id: OFFER_ID,
    version: "1.1.0",
    notes: "Adds signed updates and improves Memory history.",
    published_at: "2026-08-09T00:00:00Z",
  },
};

const openUpdateSettings = async (user: ReturnType<typeof userEvent.setup>) => {
  await user.click(screen.getByRole("button", { name: "Settings" }));
  await user.click(screen.getByRole("button", { name: "Updates" }));
};

describe("Guru Terminal · Updates", () => {
  it("loads native status and checks manually from Settings", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const status = vi
      .spyOn(bridge, "updateStatus")
      .mockResolvedValue(currentRelease);
    const check = vi
      .spyOn(bridge, "updateCheck")
      .mockResolvedValue(currentRelease);
    await openApp(bridge);

    await waitFor(() => expect(status).toHaveBeenCalled());
    expect(check).not.toHaveBeenCalled();
    await openUpdateSettings(user);
    await user.click(screen.getByRole("button", { name: "Check for updates" }));

    await waitFor(() => expect(check).toHaveBeenCalledTimes(1));
    expect(await screen.findByText("Guru Terminal is up to date")).toBeVisible();
    expect(screen.getByText("0.0.1")).toBeVisible();
  });

  it("submits only the opaque native offer after explicit install intent", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    vi.spyOn(bridge, "updateStatus").mockResolvedValue(availableRelease);
    const install = vi
      .spyOn(bridge, "updateInstall")
      .mockResolvedValue({ outcome: "cancelled", blockers: [] });
    await openApp(bridge);
    await openUpdateSettings(user);

    expect(
      await screen.findByText("Guru Terminal 1.1.0 is available"),
    ).toBeVisible();
    expect(
      screen.getByText("Adds signed updates and improves Memory history."),
    ).toBeVisible();
    expect(install).not.toHaveBeenCalled();

    await user.click(
      screen.getByRole("button", { name: "Install and restart" }),
    );
    await waitFor(() =>
      expect(install).toHaveBeenCalledWith({ offer_id: OFFER_ID }),
    );
  });

  it("shows typed active-work blockers without offering force quit", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    vi.spyOn(bridge, "updateStatus").mockResolvedValue(availableRelease);
    vi.spyOn(bridge, "updateInstall").mockResolvedValue({
      outcome: "blocked",
      blockers: [
        {
          id: "run-1",
          kind: "memory_mutation",
          label: "memory update for Guru value (session-1)",
        },
      ],
    });
    await openApp(bridge);
    await openUpdateSettings(user);

    await user.click(
      await screen.findByRole("button", { name: "Install and restart" }),
    );

    expect(
      await screen.findByText("Finish active work before updating"),
    ).toBeVisible();
    expect(
      screen.getByText("memory update for Guru value (session-1)"),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: /force/i }),
    ).not.toBeInTheDocument();
  });

  it("uses native scheduling and exposes availability without auto-installing", async () => {
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const status = vi
      .spyOn(bridge, "updateStatus")
      .mockResolvedValue(availableRelease);
    const check = vi.spyOn(bridge, "updateCheck");
    const install = vi.spyOn(bridge, "updateInstall");
    await openApp(bridge);

    await waitFor(() => expect(status).toHaveBeenCalled());
    expect(check).not.toHaveBeenCalled();
    expect(install).not.toHaveBeenCalled();
    expect((await screen.findAllByLabelText("Update available"))[0]).toBeVisible();
  });

  it("renders native download progress", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    vi.spyOn(bridge, "updateStatus").mockResolvedValue({
      ...availableRelease,
      phase: "downloading",
      downloaded_bytes: 50,
      total_bytes: 100,
    });
    await openApp(bridge);
    await openUpdateSettings(user);

    expect(
      await screen.findByText("Downloading and verifying the update… 50%"),
    ).toBeVisible();
  });
});
