import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App } from "../../App";
import { MockGuruTerminalBridge } from "../../bridge/mockBridge";
import { createMockMarketplaceSnapshot } from "../../marketplace/mockSnapshot";
import { openApp } from "../renderApp";

describe("Guru Terminal · Marketplace", () => {
  it("browses the global catalog without an Agent", async () => {
    const user = userEvent.setup();
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    vi.spyOn(bridge, "guruList").mockResolvedValueOnce([]);
    render(<App bridge={bridge} />);

    await user.click(
      await screen.findByRole("button", { name: /Marketplace/ }),
    );

    expect(
      await screen.findByRole("heading", { name: "SEC EDGAR" }),
    ).toBeVisible();
    expect(
      screen.queryByText(/Create an agent to browse/),
    ).not.toBeInTheDocument();
  });

  it("shows the bundled capability catalog", async () => {
    const user = userEvent.setup();
    await openApp();

    await user.click(screen.getByRole("button", { name: /Marketplace/ }));

    expect(screen.getByRole("heading", { name: "Marketplace" })).toBeVisible();
    expect(screen.getByText(/\d+ capabilities/)).toBeVisible();
    expect(screen.queryByText(/Adding tools for/)).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "SEC EDGAR" })).toBeVisible();
    const secCard = screen
      .getByRole("heading", { name: "SEC EDGAR" })
      .closest(".marketplace-card");
    expect(secCard).not.toBeNull();
    expect(
      within(secCard as HTMLElement).getByText("No API key required"),
    ).toBeVisible();
    expect(
      within(secCard as HTMLElement).getByRole("button", {
        name: "Set up SEC EDGAR",
      }),
    ).toBeVisible();
    expect(screen.getByRole("heading", { name: "OpenDART" })).toBeVisible();
    expect(screen.getByRole("button", { name: /Marketplace/ })).toHaveAttribute(
      "aria-current",
      "page",
    );

    const search = screen.getByRole("searchbox", {
      name: "Search Marketplace",
    });
    await user.type(search, "Korean");
    expect(screen.getByRole("heading", { name: "OpenDART" })).toBeVisible();
    expect(screen.queryByRole("heading", { name: "SEC EDGAR" })).toBeNull();

    await user.clear(search);
    await user.click(
      screen.getByRole("combobox", { name: "Filter tools by access" }),
    );
    await user.click(screen.getByRole("option", { name: "No key" }));
    expect(screen.getByRole("heading", { name: "Finance Core" })).toBeVisible();
    expect(screen.queryByRole("heading", { name: "OpenDART" })).toBeNull();
    await user.click(
      screen.getByRole("combobox", { name: "Filter tools by access" }),
    );
    await user.click(screen.getByRole("option", { name: "All" }));
    expect(
      screen.getByRole("heading", { name: "Python Compute" }),
    ).toBeVisible();
    expect(screen.getByRole("heading", { name: "Finance Core" })).toBeVisible();
    expect(
      screen.getByRole("heading", { name: "World Bank Indicators" }),
    ).toBeVisible();
    expect(
      screen.getByRole("heading", { name: "OpenBB" }),
    ).toBeVisible();
    expect(
      screen.getByRole("heading", { name: "OpenBB Platform" }),
    ).toBeVisible();
    const openbbCard = screen
      .getByRole("heading", { name: "OpenBB Platform" })
      .closest(".marketplace-card");
    expect(openbbCard).not.toBeNull();
    expect(within(openbbCard as HTMLElement).getByText("Ready")).toBeVisible();
    expect(
      within(openbbCard as HTMLElement).queryByRole("button"),
    ).toBeNull();
    expect(screen.getByRole("heading", { name: "Web Research" })).toBeVisible();
    for (const preview of ["Web Research"]) {
      const card = screen
        .getByRole("heading", { name: preview })
        .closest(".marketplace-card");
      expect(card).not.toBeNull();
      expect(within(card as HTMLElement).getByText("Preview")).toBeVisible();
    }
    expect(
      screen.getByRole("button", { name: "Settings Web Research" }),
    ).toBeVisible();
    for (const connector of [
      "SEC EDGAR",
      "OpenDART",
      "KRX Open API",
      "FRED",
      "Korea Investment Open Trading API",
      "Alpha Vantage",
    ]) {
      expect(screen.getByRole("heading", { name: connector })).toBeVisible();
    }
    expect(screen.queryByText("Built in")).not.toBeInTheDocument();
    expect(screen.queryByText("Authority")).not.toBeInTheDocument();
    expect(screen.queryByText("Network")).not.toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /Community/ })).toBeVisible();
    expect(screen.getByRole("tab", { name: /Libraries/ })).toBeVisible();
  });

  it("shows community and library sources as coming soon without install actions", async () => {
    const user = userEvent.setup();
    await openApp();
    await user.click(screen.getByRole("button", { name: /Marketplace/ }));

    await user.click(screen.getByRole("tab", { name: /Community/ }));
    expect(
      screen.getByRole("heading", { name: "Community is coming soon" }),
    ).toBeVisible();
    expect(
      screen.getByText(/Nothing is installed from this tab today/),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: /install|subscribe|add plugin/i }),
    ).not.toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: /Libraries/ }));
    expect(
      screen.getByRole("heading", { name: "Libraries is coming soon" }),
    ).toBeVisible();
    expect(
      screen.getByText(/Wiki and Lens packs over GitHub/),
    ).toBeVisible();
  });

  it("shows a missing bundled runtime instead of a dead setup action", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const snapshot = createMockMarketplaceSnapshot(
      new Set(),
      new Set(),
      new Map(),
      new Map(),
      new Map(),
      new Set(["openbb.platform"]),
    );
    vi.spyOn(bridge, "marketplaceSnapshot").mockResolvedValue(snapshot);

    await user.click(screen.getByRole("button", { name: /Marketplace/ }));

    const openbbCard = (
      await screen.findByRole("heading", { name: "OpenBB Platform" })
    ).closest(".marketplace-card");
    expect(openbbCard).not.toBeNull();
    expect(
      within(openbbCard as HTMLElement).getByText("Runtime unavailable"),
    ).toBeVisible();
    expect(
      within(openbbCard as HTMLElement).queryByText("Needs setup"),
    ).toBeNull();
    expect(
      within(openbbCard as HTMLElement).queryByRole("button"),
    ).toBeNull();
  });

  it("customizes deterministic Web Research routing without changing an Agent binding", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const configure = vi.spyOn(bridge, "marketplaceConnectorConfigure");
    const enable = vi.spyOn(bridge, "guruCapabilityEnable");
    const disable = vi.spyOn(bridge, "guruCapabilityDisable");

    await user.click(screen.getByRole("button", { name: /Marketplace/ }));
    await user.click(
      await screen.findByRole("button", { name: "Settings Web Research" }),
    );

    const dialog = screen.getByRole("dialog", {
      name: "Settings Web Research",
    });
    expect(
      within(dialog).getByText(/xAI and other providers use Exa directly/),
    ).toBeVisible();
    await user.click(
      within(dialog).getByRole("combobox", { name: "Search routing" }),
    );
    expect(screen.getByRole("option", { name: "Automatic" })).toBeVisible();
    expect(
      screen.getByRole("option", { name: "Model search only" }),
    ).toBeVisible();
    expect(screen.getByRole("option", { name: "Exa only" })).toBeVisible();
    await user.click(screen.getByRole("option", { name: "Model search only" }));
    await user.click(
      within(dialog).getByRole("button", { name: "Save setup" }),
    );

    await waitFor(() =>
      expect(configure).toHaveBeenCalledWith({
        entry_id: "community.web-research",
        config: { search_policy: "model_only" },
      }),
    );
    expect(enable).not.toHaveBeenCalled();
    expect(disable).not.toHaveBeenCalled();
    expect(await within(dialog).findByText("Settings saved")).toBeVisible();
    expect(
      (await bridge.marketplaceSnapshot()).connectors.find(
        (connector) => connector.entry_id === "community.web-research",
      )?.config,
    ).toEqual({ search_policy: "model_only" });
  });

  it("labels a partially configured connector as unfinished", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    await bridge.marketplaceConnectorConfigure({
      entry_id: "koreainvestment.market-data",
      config: { environment: "demo" },
    });

    await user.click(screen.getByRole("button", { name: /Marketplace/ }));

    expect(
      await screen.findByRole("button", {
        name: "Continue setup Korea Investment Open Trading API",
      }),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", {
        name: "Manage Korea Investment Open Trading API",
      }),
    ).not.toBeInTheDocument();
  });

  it("saves global connector configuration without changing an Agent binding", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const configure = vi.spyOn(bridge, "marketplaceConnectorConfigure");
    const enable = vi.spyOn(bridge, "guruCapabilityEnable");

    await user.click(screen.getByRole("button", { name: /Marketplace/ }));
    await user.click(
      await screen.findByRole("button", { name: "Set up SEC EDGAR" }),
    );

    const dialog = screen.getByRole("dialog", { name: "Set up SEC EDGAR" });
    expect(within(dialog).queryByText("Network")).not.toBeInTheDocument();
    const email = within(dialog).getByRole("textbox", {
      name: "SEC contact email",
    });
    expect(email).toHaveAttribute("type", "email");
    await user.type(email, "research@example.com");
    await user.click(
      within(dialog).getByRole("button", { name: "Save setup" }),
    );

    await waitFor(() =>
      expect(configure).toHaveBeenCalledWith({
        entry_id: "sec.edgar",
        config: { contact_email: "research@example.com" },
      }),
    );
    expect(enable).not.toHaveBeenCalled();
    expect(await within(dialog).findByText("Ready")).toBeVisible();
    expect(
      (await bridge.marketplaceSnapshot()).connectors.find(
        (connector) => connector.entry_id === "sec.edgar",
      )?.config,
    ).toEqual({ contact_email: "research@example.com" });

    await user.click(within(dialog).getByRole("button", { name: "Close" }));
    await waitFor(() =>
      expect(
        screen.queryByRole("dialog", { name: "Set up SEC EDGAR" }),
      ).not.toBeInTheDocument(),
    );
    await user.click(screen.getByRole("button", { name: /Agents/ }));
    const secToggle = await screen.findByRole("switch", {
      name: "SEC EDGAR: disabled",
    });
    expect(secToggle).toBeEnabled();
    await user.click(secToggle);
    expect(enable).toHaveBeenCalledWith({
      guru_id: "guru-quality",
      entry_id: "sec.edgar",
    });
  });

  it("keeps global API keys opaque across setup, verification, and deletion", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const save = vi.spyOn(bridge, "marketplaceCredentialSave");
    const verify = vi.spyOn(bridge, "marketplaceCredentialVerify");
    const enable = vi.spyOn(bridge, "guruCapabilityEnable");
    const deleteCredentialOnDevice =
      bridge.marketplaceCredentialDelete.bind(bridge);
    let releaseDelete: () => void = () => {};
    const deletePending = new Promise<void>((resolve) => {
      releaseDelete = () => resolve();
    });
    const deleteCredential = vi
      .spyOn(bridge, "marketplaceCredentialDelete")
      .mockImplementation(async (request) => {
        await deletePending;
        return deleteCredentialOnDevice(request);
      });
    const openHelp = vi
      .spyOn(bridge, "openExternalUrl")
      .mockResolvedValue(undefined);

    await user.click(screen.getByRole("button", { name: /Marketplace/ }));
    await user.click(
      await screen.findByRole("button", { name: "Set up OpenDART" }),
    );

    let dialog = screen.getByRole("dialog", { name: "Set up OpenDART" });
    let apiKey = within(dialog).getByLabelText("OpenDART API key");
    expect(apiKey).toHaveAttribute("type", "password");
    expect(apiKey).toHaveValue("");
    expect(within(dialog).getByText("Not stored")).toBeVisible();

    await user.type(apiKey, "discard-this-secret");
    await user.click(within(dialog).getByRole("button", { name: "Close" }));
    await user.click(screen.getByRole("button", { name: "Set up OpenDART" }));
    dialog = screen.getByRole("dialog", { name: "Set up OpenDART" });
    apiKey = within(dialog).getByLabelText("OpenDART API key");
    expect(apiKey).toHaveValue("");
    expect(screen.queryByDisplayValue("discard-this-secret")).toBeNull();

    await user.click(
      within(dialog).getByRole("button", {
        name: "Open OpenDART API key setup help",
      }),
    );
    expect(openHelp).toHaveBeenCalledWith(
      "https://opendart.fss.or.kr/uss/umt/EgovMberInsertView.do",
    );

    const secret = "open-dart-test-key";
    await user.type(apiKey, secret);
    await user.click(
      within(dialog).getByRole("button", {
        name: "Save & verify",
      }),
    );

    await waitFor(() =>
      expect(save).toHaveBeenCalledWith({
        entry_id: "opendart.disclosures",
        secrets: { api_key: secret },
      }),
    );
    expect(verify).toHaveBeenCalledWith({
      entry_id: "opendart.disclosures",
    });
    expect(enable).not.toHaveBeenCalled();
    expect(await within(dialog).findByText("Ready")).toBeVisible();
    expect(apiKey).toHaveValue("");
    expect(screen.queryByDisplayValue(secret)).toBeNull();
    expect(within(dialog).getByText(/Stored securely/)).toBeVisible();

    await user.click(
      within(dialog).getByRole("button", {
        name: "Delete saved credentials",
      }),
    );
    const confirmation = screen.getByRole("dialog", {
      name: "Delete saved credentials?",
    });
    expect(deleteCredential).not.toHaveBeenCalled();
    expect(
      within(confirmation).getByText(/from every agent that uses/),
    ).toBeVisible();
    await user.click(
      within(confirmation).getByRole("button", { name: "Delete credentials" }),
    );
    await waitFor(() =>
      expect(deleteCredential).toHaveBeenCalledWith({
        entry_id: "opendart.disclosures",
      }),
    );
    expect(
      within(confirmation).getByRole("button", { name: "Deleting…" }),
    ).toBeDisabled();
    await user.click(
      within(confirmation).getByRole("button", { name: "Close" }),
    );
    expect(
      screen.queryByRole("dialog", { name: "Delete saved credentials?" }),
    ).not.toBeInTheDocument();
    expect(dialog).toBeVisible();

    await act(async () => releaseDelete());
    expect(await within(dialog).findByText("Not stored")).toBeVisible();
    expect(
      within(dialog).getByRole("button", { name: "Save & verify" }),
    ).toBeEnabled();
  });

  it("stages required KIS credentials and optional profile patches without exposing saved values", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    const configure = vi.spyOn(bridge, "marketplaceConnectorConfigure");
    const save = vi.spyOn(bridge, "marketplaceCredentialSave");
    const verify = vi.spyOn(bridge, "marketplaceCredentialVerify");

    await user.click(screen.getByRole("button", { name: /Marketplace/ }));
    await user.click(
      await screen.findByRole("button", {
        name: "Set up Korea Investment Open Trading API",
      }),
    );

    const dialog = screen.getByRole("dialog", {
      name: "Set up Korea Investment Open Trading API",
    });
    const environment = within(dialog).getByRole("combobox", {
      name: "Account type",
    });
    expect(environment).toHaveTextContent("Live");
    const appKey = within(dialog).getByLabelText("KIS app key");
    const appSecret = within(dialog).getByLabelText("KIS app secret");
    const accountNumber = within(dialog).getByLabelText(
      "KIS account number (optional)",
    );
    const accountProductCode = within(dialog).getByLabelText(
      "KIS account product code (optional)",
    );
    const htsId = within(dialog).getByLabelText("KIS HTS ID (optional)");
    expect(accountNumber).toHaveAttribute("type", "password");
    expect(accountProductCode).toHaveAttribute("type", "password");
    expect(htsId).toHaveAttribute("type", "password");
    expect(within(dialog).getByText(/stay on this device/)).toBeVisible();
    expect(within(dialog).getByText(/never sent in chat/)).toBeVisible();
    await user.type(appKey, "  test-app-key  ");
    await user.click(
      within(dialog).getByRole("button", { name: "Save & verify" }),
    );

    expect(save).not.toHaveBeenCalled();
    expect(verify).not.toHaveBeenCalled();
    expect(appSecret).toBeInvalid();

    await user.type(appSecret, "  test-app-secret  ");
    await user.click(
      within(dialog).getByRole("button", { name: "Save & verify" }),
    );

    await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(save).toHaveBeenCalledWith({
      entry_id: "koreainvestment.market-data",
      secrets: {
        app_key: "test-app-key",
        app_secret: "test-app-secret",
      },
    });
    expect(configure).toHaveBeenCalledWith({
      entry_id: "koreainvestment.market-data",
      config: { environment: "real" },
    });
    expect(verify).toHaveBeenCalledTimes(1);
    expect(verify).toHaveBeenCalledWith({
      entry_id: "koreainvestment.market-data",
    });
    expect(await within(dialog).findByText("Ready")).toBeVisible();
    expect(within(dialog).getByRole("status")).toHaveTextContent(
      "Verification successful",
    );
    expect(within(dialog).getAllByText(/Stored securely/)).toHaveLength(2);
    expect(
      (await bridge.marketplaceSnapshot()).connectors.find(
        (connector) => connector.entry_id === "koreainvestment.market-data",
      )?.config,
    ).toEqual({ environment: "real" });

    await user.type(accountNumber, "12345678");
    await user.type(accountProductCode, "01");
    await user.click(
      within(dialog).getByRole("button", { name: "Save & verify" }),
    );
    await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
    expect(save).toHaveBeenLastCalledWith({
      entry_id: "koreainvestment.market-data",
      secrets: {
        account_number: "12345678",
        account_product_code: "01",
      },
    });
    expect(verify).toHaveBeenCalledTimes(2);
    expect(accountNumber).toHaveValue("");
    expect(accountProductCode).toHaveValue("");
    expect(screen.queryByDisplayValue("12345678")).toBeNull();
    expect(within(dialog).getAllByText(/Stored securely/)).toHaveLength(4);

    await user.click(environment);
    await user.click(screen.getByRole("option", { name: "Demo" }));
    await user.click(
      within(dialog).getByRole("button", { name: "Save & verify" }),
    );
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "KIS app key is required.",
    );
    expect(configure).toHaveBeenCalledTimes(2);

    await user.type(appKey, "replacement-app-key");
    await user.click(
      within(dialog).getByRole("button", { name: "Save & verify" }),
    );
    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "KIS app secret is required.",
    );
    expect(save).toHaveBeenCalledTimes(2);
    expect(verify).toHaveBeenCalledTimes(2);
  });

  it("shows a clear verification result with the safe provider diagnostic", async () => {
    const user = userEvent.setup();
    const bridge = await openApp();
    vi.spyOn(bridge, "marketplaceCredentialVerify").mockRejectedValueOnce({
      code: "credential_rejected",
      message: "KIS error · EGW00123: The app key and secret do not match.",
    });

    await user.click(screen.getByRole("button", { name: /Marketplace/ }));
    await user.click(
      await screen.findByRole("button", {
        name: "Set up Korea Investment Open Trading API",
      }),
    );
    const dialog = screen.getByRole("dialog", {
      name: "Set up Korea Investment Open Trading API",
    });
    await user.type(
      within(dialog).getByLabelText("KIS app key"),
      "test-app-key",
    );
    await user.type(
      within(dialog).getByLabelText("KIS app secret"),
      "test-app-secret",
    );
    await user.click(
      within(dialog).getByRole("button", { name: "Save & verify" }),
    );

    const alert = await within(dialog).findByRole("alert");
    expect(alert).toHaveTextContent("Verification failed");
    expect(alert).toHaveTextContent(
      "KIS error · EGW00123: The app key and secret do not match.",
    );
    expect(alert).not.toHaveTextContent("test-app-secret");
  });

  it("offers settings only when a capability declares setup", async () => {
    const user = userEvent.setup();
    await openApp();
    await user.click(screen.getByRole("button", { name: /Marketplace/ }));
    const openbbCard = () =>
      screen
        .getByRole("heading", { name: "OpenBB Platform" })
        .closest<HTMLElement>('[data-slot="card"]');
    expect(openbbCard()).not.toBeNull();
    expect(
      within(openbbCard()!).queryByRole("button", {
        name: /Set up|Manage/,
      }),
    ).not.toBeInTheDocument();
    expect(within(openbbCard()!).queryByText("Built in")).toBeNull();
    expect(
      screen.getByRole("button", { name: "Settings Web Research" }),
    ).toBeVisible();
  });
});
