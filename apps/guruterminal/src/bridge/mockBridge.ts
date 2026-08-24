import type * as Contract from "../types";
import {
  createMockGuruCapabilityBindings,
  createMockMarketplaceSnapshot,
  isValidMockSetupValue,
} from "../marketplace/mockSnapshot";
import {
  httpAddressError,
  parseCredentialFreeHttpUrl,
} from "../lib/credentialFreeUrl";
import * as guruChat from "./mock/guruChat";
import * as library from "./mock/library";
import { createMockBridgeState, type MockBridgeState } from "./mock/state";

const mockProvider = (
  id: string,
  label: string,
  credential_label: string,
  options: {
    description: string;
    api_key?: boolean;
    oauth?: string | null;
    credential_source?: Contract.ModelProviderOption["credential_source"];
    recommended?: boolean;
  },
): Contract.ModelProviderOption => ({
  id,
  label,
  credential_label,
  description: options.description,
  api_key: options.api_key ?? true,
  oauth: options.oauth ? { label: options.oauth } : null,
  credential_source: options.credential_source ?? "missing",
  recommended: options.recommended ?? false,
});

const MOCK_PERFORMANCE_CONTROL: Contract.ModelRunControl = {
  id: "performance",
  label: "Performance",
  default_choice: "standard",
  choices: [
    {
      id: "standard",
      label: "Standard",
      description: "Use the provider's standard service tier.",
    },
    {
      id: "fast",
      label: "Fast",
      description: "Request the provider's priority service tier.",
    },
  ],
};

export class MockGuruTerminalBridge implements Contract.GuruTerminalBridge {
  private readonly state: MockBridgeState;
  private modelCatalog: Contract.ModelCatalog = {
    hidden_model_profile_ids: [],
    models: [
      {
        id: "model-test",
        name: "GPT-5.6 Luna",
        provider: "openai-codex",
        model: "gpt-5.6-luna",
        input: ["text"],
        reasoning: true,
        context_window: 128_000,
        max_tokens: 32_000,
        thinking_levels: ["low", "medium", "high", "max"],
        thinking_level_map: { max: "max" },
        run_controls: [MOCK_PERFORMANCE_CONTROL],
        credential_source: "saved",
      },
    ],
    providers: [
      mockProvider("openai-codex", "OpenAI with ChatGPT", "ChatGPT account", {
        description: "Use your ChatGPT account.",
        api_key: false,
        oauth: "Continue with ChatGPT",
        credential_source: "saved",
        recommended: true,
      }),
      mockProvider("anthropic", "Anthropic", "Anthropic API key", {
        description: "Use Claude Pro or Max, or an Anthropic API key.",
        oauth: "Continue with Claude",
      }),
      mockProvider("openai", "OpenAI API", "OpenAI API key", {
        description: "Connect with an API key stored by Pi in Guru Terminal's private app data.",
        credential_source: "saved",
      }),
      mockProvider("google", "Google Gemini", "Gemini API key", {
        description: "Connect with an API key stored by Pi in Guru Terminal's private app data.",
      }),
      mockProvider("xai", "xAI", "xAI API key", {
        description: "Use SuperGrok or an xAI API key.",
        oauth: "Continue with SuperGrok",
      }),
      mockProvider("openrouter", "OpenRouter", "OpenRouter API key", {
        description: "Sign in with OpenRouter or store an API key.",
        oauth: "Continue with OpenRouter",
      }),
      mockProvider("vercel-ai-gateway", "Vercel AI Gateway", "AI Gateway API key", {
        description: "Connect with an API key stored by Pi in Guru Terminal's private app data.",
      }),
    ],
  };
  private readonly disabledCapabilities = new Map<string, Set<string>>();
  private readonly enabledCapabilities = new Map<string, Set<string>>();
  private readonly financeCredentials = new Set<string>();
  private readonly pendingFinanceCredentials = new Set<string>();
  private readonly financeCredentialFields = new Map<string, Set<string>>();
  private readonly pendingFinanceCredentialFields = new Map<
    string,
    Set<string>
  >();
  private readonly capabilityConfigs = new Map<
    string,
    Record<string, string>
  >();
  private readonly browserTabs = new Map<
    string,
    {
      state: Contract.BrowserTabState;
      observer: Contract.StreamObserver<Contract.BrowserTabEvent>;
    }
  >();

  constructor(options: { delay_ms?: number } = {}) {
    this.state = createMockBridgeState(options.delay_ms ?? 110);
  }

  /** Adjusts pacing of in-flight mock streams so tests can hold a run open deterministically. */
  setStreamDelay(delay_ms: number): void {
    this.state.delay_ms = delay_ms;
  }

  async modelCatalogGet(): Promise<Contract.ModelCatalog> {
    return structuredClone(this.modelCatalog);
  }

  async modelVisibilityUpdate(
    request: Contract.ModelVisibilityUpdateRequest,
  ): Promise<Contract.ModelCatalog> {
    const modelExists = this.modelCatalog.models.some(
      (model) => model.id === request.model_profile_id,
    );
    if (!modelExists) throw new Error("The selected model profile was not found.");
    const hidden = new Set(this.modelCatalog.hidden_model_profile_ids);
    if (request.visible_in_chat) hidden.delete(request.model_profile_id);
    else hidden.add(request.model_profile_id);
    this.modelCatalog.hidden_model_profile_ids = [...hidden].sort();
    return structuredClone(this.modelCatalog);
  }

  private executionModelLock(
    model_profile_id: string,
    thinking_level: string,
    run_options: Record<string, string>,
  ): Contract.ExecutionModelLock {
    const model = this.modelCatalog.models.find(
      (item) => item.id === model_profile_id,
    );
    if (!model) throw new Error("The selected model profile was not found.");
    if (this.modelCatalog.hidden_model_profile_ids.includes(model.id)) {
      throw new Error("The selected model is hidden from Chat.");
    }
    if (!model.thinking_levels.includes(thinking_level)) {
      throw new Error(
        "The selected thinking level is not available for this model.",
      );
    }
    return {
      profile_id: model.id,
      name: model.name,
      provider: model.provider,
      model: model.model,
      thinking_level,
      run_options,
    };
  }

  async providerModels(provider: string): Promise<Contract.ModelCatalog> {
    const catalog: Record<string, Contract.ProviderModelOption[]> = {
      "openai-codex": [
        {
          id: "gpt-5.6-luna",
          name: "GPT-5.6 Luna",
          reasoning: true,
          context_window: 272_000,
          max_tokens: 128_000,
          input: ["text"],
          thinking_levels: ["low", "medium", "high", "max"],
          thinking_level_map: { max: "max" },
          run_controls: [MOCK_PERFORMANCE_CONTROL],
        },
        {
          id: "gpt-5.6-sol",
          name: "GPT-5.6 Sol",
          reasoning: true,
          context_window: 272_000,
          max_tokens: 128_000,
          input: ["text"],
          thinking_levels: ["off", "low", "medium", "high", "xhigh"],
          thinking_level_map: { xhigh: "xhigh" },
          run_controls: [MOCK_PERFORMANCE_CONTROL],
        },
        {
          id: "gpt-5.6-terra",
          name: "GPT-5.6 Terra",
          reasoning: true,
          context_window: 272_000,
          max_tokens: 128_000,
          input: ["text"],
          thinking_levels: ["off", "low", "medium", "high"],
          thinking_level_map: {},
          run_controls: [MOCK_PERFORMANCE_CONTROL],
        },
      ],
      anthropic: [
        {
          id: "claude-sonnet-4-5",
          name: "Claude Sonnet 4.5",
          reasoning: true,
          context_window: 200_000,
          max_tokens: 64_000,
          input: ["text", "image"],
          thinking_levels: ["off", "low", "medium", "high"],
          thinking_level_map: {},
          run_controls: [],
        },
      ],
    };
    const options = catalog[provider] ?? [
      {
        id: `${provider}-default`,
        name: `${provider} default`,
        reasoning: false,
        context_window: 128_000,
        max_tokens: 32_000,
        input: ["text"],
        thinking_levels: ["off"],
        thinking_level_map: {},
        run_controls: [],
      },
    ];
    const credentialSource =
      this.modelCatalog.providers.find((item) => item.id === provider)
        ?.credential_source ?? "missing";
    this.modelCatalog.models = [
      ...this.modelCatalog.models.filter(
        (model) => model.provider !== provider,
      ),
      ...options.map((option) => ({
        id: `${provider}/${option.id}`,
        name: option.name,
        provider,
        model: option.id,
        input: option.input,
        reasoning: option.reasoning,
        context_window: option.context_window,
        max_tokens: option.max_tokens,
        thinking_levels: option.thinking_levels,
        thinking_level_map: option.thinking_level_map,
        run_controls: option.run_controls,
        credential_source: credentialSource,
      })),
    ];
    return structuredClone(this.modelCatalog);
  }

  async providerConfigure(request: Contract.ProviderConfigureRequest) {
    const option = this.modelCatalog.providers.find(
      (item) => item.id === request.provider,
    );
    if (option)
      option.credential_source = request.clear_saved_key ? "missing" : "saved";
    return this.providerModels(request.provider);
  }

  async providerConnect(
    provider: string,
    observer: Contract.StreamObserver<Contract.ProviderConnectionEvent>,
  ) {
    const option = this.modelCatalog.providers.find(
      (item) => item.id === provider,
    );
    observer({
      type: "opening_browser",
      message: "A secure sign-in page was opened.",
    });
    observer({
      type: "connected",
      message: `${option?.label ?? "Provider"} is connected.`,
    });
    if (option) option.credential_source = "saved";
    return this.providerModels(provider);
  }

  async providerConnectCancel() {}

  async providerConnectOpenBrowser() {}

  async providerDisconnect(provider: string) {
    const option = this.modelCatalog.providers.find(
      (item) => item.id === provider,
    );
    if (option) option.credential_source = "missing";
    this.modelCatalog.models = this.modelCatalog.models.filter(
      (model) => model.provider !== provider,
    );
    return structuredClone(this.modelCatalog);
  }

  async marketplaceSnapshot(): Promise<Contract.MarketplaceSnapshot> {
    return createMockMarketplaceSnapshot(
      this.financeCredentials,
      this.pendingFinanceCredentials,
      this.capabilityConfigs,
      this.financeCredentialFields,
      this.pendingFinanceCredentialFields,
    );
  }

  async guruCapabilityList(
    guru_id: string,
  ): Promise<Contract.GuruCapabilityBinding[]> {
    return createMockGuruCapabilityBindings(
      this.disabledCapabilities.get(guru_id) ?? new Set(),
      this.enabledCapabilities.get(guru_id) ?? new Set(),
      await this.marketplaceSnapshot(),
    );
  }

  agentSkillCatalog(guru_id: string): Promise<Contract.AgentSkillSummary[]> {
    return guruChat.agentSkillCatalog(this.state, guru_id);
  }

  agentSkillsUpdate(
    request: Contract.AgentSkillsUpdateRequest,
  ): Promise<Contract.GuruSummary> {
    return guruChat.agentSkillsUpdate(this.state, request);
  }

  async guruCapabilityEnable(request: Contract.GuruCapabilityRequest) {
    this.disabledCapabilities.get(request.guru_id)?.delete(request.entry_id);
    const enabled =
      this.enabledCapabilities.get(request.guru_id) ?? new Set<string>();
    enabled.add(request.entry_id);
    this.enabledCapabilities.set(request.guru_id, enabled);
    const binding = (await this.guruCapabilityList(request.guru_id)).find(
      (candidate) => candidate.entry_id === request.entry_id,
    );
    if (!binding) throw new Error("Capability binding is unavailable.");
    return binding;
  }

  async marketplaceConnectorConfigure(
    request: Contract.MarketplaceConnectorConfigureRequest,
  ) {
    const entry = createMockMarketplaceSnapshot().catalog.entries.find(
      (candidate) => candidate.id === request.entry_id,
    );
    const fields = entry?.setup?.config_fields ?? [];
    if (
      Object.keys(request.config).some(
        (fieldId) => !fields.some((field) => field.id === fieldId),
      ) ||
      fields.some((field) => {
        const value = request.config[field.id];
        return value === undefined
          ? field.required
          : !isValidMockSetupValue(field, value);
      })
    ) {
      throw new Error(
        "Configuration does not match the connector setup contract.",
      );
    }
    const scopeFields = entry?.setup?.credential_scope_fields ?? [];
    if (
      scopeFields.some(
        (fieldId) =>
          this.capabilityConfigs.get(request.entry_id)?.[fieldId] !==
          request.config[fieldId],
      )
    ) {
      this.financeCredentials.delete(request.entry_id);
      this.pendingFinanceCredentials.delete(request.entry_id);
      this.financeCredentialFields.delete(request.entry_id);
      this.pendingFinanceCredentialFields.delete(request.entry_id);
      for (const [guruId, enabled] of this.enabledCapabilities) {
        enabled.delete(request.entry_id);
        const disabled =
          this.disabledCapabilities.get(guruId) ?? new Set<string>();
        disabled.add(request.entry_id);
        this.disabledCapabilities.set(guruId, disabled);
      }
    }
    this.capabilityConfigs.set(
      request.entry_id,
      structuredClone(request.config),
    );
  }

  async guruCapabilityDisable(request: Contract.GuruCapabilityRequest) {
    const disabled =
      this.disabledCapabilities.get(request.guru_id) ?? new Set<string>();
    disabled.add(request.entry_id);
    this.disabledCapabilities.set(request.guru_id, disabled);
    this.enabledCapabilities.get(request.guru_id)?.delete(request.entry_id);
    const binding = (await this.guruCapabilityList(request.guru_id)).find(
      (candidate) => candidate.entry_id === request.entry_id,
    );
    if (!binding) throw new Error("Capability binding is unavailable.");
    return binding;
  }

  async marketplaceCredentialSave(
    request: Contract.MarketplaceCredentialSaveRequest,
  ) {
    const entry = createMockMarketplaceSnapshot().catalog.entries.find(
      (candidate) => candidate.id === request.entry_id,
    );
    const fields = entry?.setup?.credential_fields ?? [];
    const submittedIds = Object.keys(request.secrets).sort();
    const declaredIds = new Set(fields.map((field) => field.id));
    const existingFields = new Set([
      ...(this.financeCredentialFields.get(request.entry_id) ?? []),
      ...(this.pendingFinanceCredentialFields.get(request.entry_id) ?? []),
    ]);
    const mergedFields = new Set([...existingFields, ...submittedIds]);
    if (
      !fields.length ||
      !submittedIds.length ||
      submittedIds.some((credentialId) => !declaredIds.has(credentialId)) ||
      submittedIds.some((credentialId) => {
        const field = fields.find((candidate) => candidate.id === credentialId);
        return !field || !isValidMockSetupValue(field, request.secrets[credentialId] ?? "");
      }) ||
      fields.some((field) => field.required && !mergedFields.has(field.id))
    ) {
      throw new Error("Credentials do not match the connector setup contract.");
    }
    this.pendingFinanceCredentials.add(request.entry_id);
    this.pendingFinanceCredentialFields.set(request.entry_id, mergedFields);
    return (await this.marketplaceSnapshot()).connectors.find(
      (connector) => connector.entry_id === request.entry_id,
    )!
      .credentials;
  }

  async marketplaceCredentialVerify(
    request: Contract.MarketplaceCredentialRequest,
  ) {
    if (
      !this.pendingFinanceCredentials.has(request.entry_id) &&
      !this.financeCredentials.has(request.entry_id)
    ) {
      throw new Error("Credential is missing.");
    }
    const pendingFields = this.pendingFinanceCredentialFields.get(
      request.entry_id,
    );
    if (pendingFields) {
      this.financeCredentialFields.set(
        request.entry_id,
        new Set(pendingFields),
      );
    }
    this.pendingFinanceCredentials.delete(request.entry_id);
    this.pendingFinanceCredentialFields.delete(request.entry_id);
    this.financeCredentials.add(request.entry_id);
    return (await this.marketplaceSnapshot()).connectors.find(
      (connector) => connector.entry_id === request.entry_id,
    )!
      .credentials;
  }

  async marketplaceCredentialDelete(
    request: Contract.MarketplaceCredentialRequest,
  ) {
    this.financeCredentials.delete(request.entry_id);
    this.pendingFinanceCredentials.delete(request.entry_id);
    this.financeCredentialFields.delete(request.entry_id);
    this.pendingFinanceCredentialFields.delete(request.entry_id);
    for (const [guruId, enabled] of this.enabledCapabilities) {
      enabled.delete(request.entry_id);
      const disabled =
        this.disabledCapabilities.get(guruId) ?? new Set<string>();
      disabled.add(request.entry_id);
      this.disabledCapabilities.set(guruId, disabled);
    }
    return (await this.marketplaceSnapshot()).connectors.find(
      (connector) => connector.entry_id === request.entry_id,
    )!
      .credentials;
  }

  async openExternalUrl(url: string) {
    if (new TextEncoder().encode(url).byteLength > 8 * 1024) {
      throw new Error("External link is too long.");
    }
    const parsed = parseCredentialFreeHttpUrl(url);
    if (!parsed) {
      throw new Error(httpAddressError);
    }
    window.open(parsed.href, "_blank", "noopener,noreferrer");
  }

  async browserTabOpen(
    request: Contract.BrowserTabOpenRequest,
    observer: Contract.StreamObserver<Contract.BrowserTabEvent>,
  ) {
    if (this.browserTabs.size >= 12) {
      throw new Error("Close a browser tab before opening another one.");
    }
    const parsed = this.validBrowserUrl(request.url);
    const tab_id = crypto.randomUUID();
    const state: Contract.BrowserTabState = {
      tab_id,
      url: parsed.href,
      title: parsed.hostname,
      loading: false,
    };
    this.browserTabs.set(tab_id, { state, observer });
    queueMicrotask(() => {
      observer({ type: "load_finished", tab_id, url: parsed.href });
      observer({ type: "title_changed", tab_id, title: parsed.hostname });
    });
    return structuredClone(state);
  }

  async browserTabNavigate(tab_id: string, url: string) {
    const tab = this.browserTabs.get(tab_id);
    if (!tab) throw new Error("Browser tab was not found.");
    const parsed = this.validBrowserUrl(url);
    tab.state = { ...tab.state, url: parsed.href, title: parsed.hostname };
    tab.observer({ type: "load_started", tab_id, url: parsed.href });
    tab.observer({ type: "load_finished", tab_id, url: parsed.href });
    tab.observer({ type: "title_changed", tab_id, title: parsed.hostname });
  }

  async browserTabHistory(
    tab_id: string,
    _direction: Contract.BrowserHistoryDirection,
  ) {
    if (!this.browserTabs.has(tab_id))
      throw new Error("Browser tab was not found.");
  }

  async browserTabReload(tab_id: string) {
    const tab = this.browserTabs.get(tab_id);
    if (!tab) throw new Error("Browser tab was not found.");
    tab.observer({ type: "load_started", tab_id, url: tab.state.url });
    tab.observer({ type: "load_finished", tab_id, url: tab.state.url });
  }

  async browserTabSetBounds(request: Contract.BrowserTabBoundsRequest) {
    if (!this.browserTabs.has(request.tab_id)) {
      throw new Error("Browser tab was not found.");
    }
  }

  async browserTabClose(tab_id: string) {
    if (!this.browserTabs.delete(tab_id))
      throw new Error("Browser tab was not found.");
  }

  async browserTabsReset() {
    this.browserTabs.clear();
  }

  private validBrowserUrl(raw: string) {
    if (new TextEncoder().encode(raw).byteLength > 8 * 1024) {
      throw new Error("External link is too long.");
    }
    const parsed = parseCredentialFreeHttpUrl(raw);
    if (!parsed) {
      throw new Error(httpAddressError);
    }
    return parsed;
  }

  async updateStatus(): Promise<Contract.UpdateState> {
    return {
      supported: true,
      current_version: "0.0.1",
      phase: "idle",
      offer: null,
      downloaded_bytes: 0,
      total_bytes: null,
      last_checked_at_ms: null,
      next_auto_check_at_ms: null,
      error: null,
      blockers: [],
    };
  }

  updateCheck(): Promise<Contract.UpdateState> {
    return this.updateStatus();
  }

  async updateInstall(
    _request: Contract.UpdateInstallRequest,
  ): Promise<Contract.UpdateInstallResult> {
    return { outcome: "cancelled", blockers: [] };
  }

  guruList(): Promise<Contract.GuruSummary[]> {
    return guruChat.guruList(this.state);
  }

  guruSelect(guru_id: string): Promise<Contract.GuruWorkspace> {
    return guruChat.guruSelect(this.state, guru_id);
  }

  async guruRecover(
    request: Contract.GuruRecoverRequest,
  ): Promise<Contract.GuruSummary> {
    const guru = this.state.gurus.find(
      (candidate) => candidate.id === request.guru_id,
    );
    if (!guru) throw new Error("Guru not found");
    if (request.action !== "recover_memory") {
      throw new Error("Unsupported Guru recovery action");
    }
    guru.availability = { status: "available" };
    return structuredClone(guru);
  }

  guruCreate(
    request: Contract.GuruCreateRequest,
  ): Promise<Contract.GuruSummary> {
    return guruChat.guruCreate(this.state, request);
  }

  guruImportMemory(): Promise<Contract.GuruSummary> {
    return guruChat.guruImport(this.state);
  }

  async guruExportMemory(guru_id: string): Promise<Contract.GuruExportReceipt> {
    return { guru_id, record_count: 0, memory_revision: "mock-revision" };
  }

  guruRename(request: Contract.GuruRenameRequest) {
    return guruChat.guruRename(this.state, request);
  }

  async guruDelete(request: Contract.GuruDeleteRequest) {
    await guruChat.guruDelete(this.state, request);
    this.disabledCapabilities.delete(request.guru_id);
  }

  chatCreate(
    request: Contract.ChatCreateRequest,
  ): Promise<Contract.ChatThread> {
    return guruChat.chatCreate(this.state, request);
  }

  chatRename(
    request: Contract.ChatRenameRequest,
  ): Promise<Contract.ChatThread> {
    return guruChat.chatRename(this.state, request);
  }

  chatDelete(request: Contract.ChatDeleteRequest): Promise<void> {
    return guruChat.chatDelete(this.state, request);
  }

  chatAttachmentRead(
    guru_id: string,
    thread_id: string,
    message_id: string,
    attachment_id: string,
  ) {
    return guruChat.chatAttachmentRead(
      this.state,
      guru_id,
      thread_id,
      message_id,
      attachment_id,
    );
  }

  chatArtifactList(guru_id: string, thread_id: string) {
    return guruChat.chatArtifactList(this.state, guru_id, thread_id);
  }

  chatArtifactRead(
    guru_id: string,
    thread_id: string,
    artifact_id: string,
  ) {
    return guruChat.chatArtifactRead(this.state, guru_id, thread_id, artifact_id);
  }

  chatSend(
    request: Contract.ChatSendRequest,
    observer: Contract.StreamObserver<Contract.ChatStreamEvent>,
  ): Promise<{ run_id: string }> {
    const capabilities = createMockGuruCapabilityBindings(
      this.disabledCapabilities.get(request.guru_id) ?? new Set(),
      this.enabledCapabilities.get(request.guru_id) ?? new Set(),
      createMockMarketplaceSnapshot(
        this.financeCredentials,
        this.pendingFinanceCredentials,
        this.capabilityConfigs,
        this.financeCredentialFields,
        this.pendingFinanceCredentialFields,
      ),
    ).flatMap((binding) => (binding.enabled ? [binding.entry_id] : []));
    return guruChat.chatSend(
      this.state,
      request,
      observer,
      capabilities,
      this.executionModelLock(
        request.model_profile_id,
        request.thinking_level,
        request.run_options,
      ),
    );
  }

  chatSteer(request: Contract.ChatControlRequest) {
    return guruChat.chatSteer(this.state, request);
  }

  chatAbort(run_id: string): Promise<void> {
    return guruChat.chatAbort(this.state, run_id);
  }

  async runActivityList(): Promise<Contract.RunActivity[]> {
    return structuredClone(
      [...this.state.run_activities.values()].sort(
        (left, right) => left.started_at_ms - right.started_at_ms,
      ),
    );
  }

  librarySearch(
    request: Contract.LibrarySearchRequest,
  ): Promise<Contract.LibrarySummary[]> {
    return library.librarySearch(this.state, request);
  }

  libraryRead(
    guru_id: string,
    record_id: string,
  ): Promise<Contract.LibraryRecord> {
    return library.libraryRead(this.state, guru_id, record_id);
  }

  libraryMemoryCreate(request: Contract.LibraryMemoryCreateRequest) {
    return library.libraryMemoryCreate(this.state, request);
  }

  libraryMemoryUpdate(request: Contract.LibraryMemoryUpdateRequest) {
    return library.libraryMemoryUpdate(this.state, request);
  }

  libraryMemoryDelete(request: Contract.LibraryMemoryDeleteRequest) {
    return library.libraryMemoryDelete(this.state, request);
  }

  libraryMemoryRevert(request: Contract.LibraryMemoryRevertRequest) {
    return library.libraryMemoryRevert(this.state, request);
  }

}
