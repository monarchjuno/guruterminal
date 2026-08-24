import type {
  AgentSkillSummary,
  AgentSkillsUpdateRequest,
  BrowserHistoryDirection,
  BrowserTabBoundsRequest,
  BrowserTabEvent,
  BrowserTabOpenRequest,
  BrowserTabState,
  ChatCreateRequest,
  ChatControlReceipt,
  ChatControlRequest,
  ChatDeleteRequest,
  ChatArtifact,
  ChatArtifactView,
  ChatRenameRequest,
  ChatSendRequest,
  ChatStreamEvent,
  ChatThread,
  GuruCapabilityBinding,
  GuruCapabilityRequest,
  GuruCreateRequest,
  GuruDeleteRequest,
  GuruExportReceipt,
  GuruRecoverRequest,
  GuruTerminalBridge,
  GuruRenameRequest,
  GuruSummary,
  GuruWorkspace,
  LibraryRecord,
  LibraryMemoryCreateRequest,
  LibraryMemoryDeleteRequest,
  LibraryMemoryMutation,
  LibraryMemoryRevertRequest,
  LibraryMemoryUpdateRequest,
  LibrarySearchRequest,
  LibrarySummary,
  MarketplaceSnapshot,
  MarketplaceCredentialStatus,
  MarketplaceCredentialRequest,
  MarketplaceCredentialSaveRequest,
  MarketplaceConnectorConfigureRequest,
  ModelCatalog,
  ModelVisibilityUpdateRequest,
  ProviderConfigureRequest,
  ProviderConnectionEvent,
  RunActivity,
  StreamObserver,
  UpdateInstallRequest,
  UpdateInstallResult,
  UpdateState,
} from "../types";
import { TAURI_COMMANDS, TAURI_STREAM_CHANNEL_ARGUMENT } from "./commands";

export class TauriGuruTerminalBridge implements GuruTerminalBridge {
  private async invoke<T>(command: string, args?: Record<string, unknown>) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>(command, args);
  }

  modelCatalogGet() {
    return this.invoke<ModelCatalog>(TAURI_COMMANDS.modelCatalogGet);
  }

  modelVisibilityUpdate(request: ModelVisibilityUpdateRequest) {
    return this.invoke<ModelCatalog>(TAURI_COMMANDS.modelVisibilityUpdate, {
      request,
    });
  }

  providerModels(provider: string) {
    return this.invoke<ModelCatalog>(TAURI_COMMANDS.providerModels, {
      request: { provider },
    });
  }

  providerConfigure(request: ProviderConfigureRequest) {
    return this.invoke<ModelCatalog>(TAURI_COMMANDS.providerConfigure, { request });
  }

  async providerConnect(
    provider: string,
    observer: StreamObserver<ProviderConnectionEvent>,
  ) {
    const { Channel, invoke } = await import("@tauri-apps/api/core");
    const onEvent = new Channel<ProviderConnectionEvent>();
    onEvent.onmessage = observer;
    return invoke<ModelCatalog>(TAURI_COMMANDS.providerConnect, {
      request: { provider },
      [TAURI_STREAM_CHANNEL_ARGUMENT]: onEvent,
    });
  }

  providerConnectCancel() {
    return this.invoke<void>(TAURI_COMMANDS.providerConnectCancel);
  }

  providerConnectOpenBrowser() {
    return this.invoke<void>(TAURI_COMMANDS.providerConnectOpenBrowser);
  }

  providerDisconnect(provider: string) {
    return this.invoke<ModelCatalog>(TAURI_COMMANDS.providerDisconnect, {
      request: { provider },
    });
  }

  marketplaceSnapshot() {
    return this.invoke<MarketplaceSnapshot>(TAURI_COMMANDS.marketplaceSnapshot);
  }

  guruCapabilityList(guru_id: string) {
    return this.invoke<GuruCapabilityBinding[]>(TAURI_COMMANDS.guruCapabilityList, {
      guru_id,
    });
  }

  agentSkillCatalog(guru_id: string) {
    return this.invoke<AgentSkillSummary[]>(TAURI_COMMANDS.agentSkillCatalog, {
      guru_id,
    });
  }

  agentSkillsUpdate(request: AgentSkillsUpdateRequest) {
    return this.invoke<GuruSummary>(TAURI_COMMANDS.agentSkillsUpdate, {
      request,
    });
  }

  guruCapabilityEnable(request: GuruCapabilityRequest) {
    return this.invoke<GuruCapabilityBinding>(TAURI_COMMANDS.guruCapabilityEnable, {
      request,
    });
  }

  marketplaceConnectorConfigure(request: MarketplaceConnectorConfigureRequest) {
    return this.invoke<void>(TAURI_COMMANDS.marketplaceConnectorConfigure, {
      request,
    });
  }

  guruCapabilityDisable(request: GuruCapabilityRequest) {
    return this.invoke<GuruCapabilityBinding>(TAURI_COMMANDS.guruCapabilityDisable, {
      request,
    });
  }

  marketplaceCredentialSave(request: MarketplaceCredentialSaveRequest) {
    return this.invoke<MarketplaceCredentialStatus[]>(
      TAURI_COMMANDS.marketplaceCredentialSave,
      { request },
    );
  }

  marketplaceCredentialVerify(request: MarketplaceCredentialRequest) {
    return this.invoke<MarketplaceCredentialStatus[]>(
      TAURI_COMMANDS.marketplaceCredentialVerify,
      { request },
    );
  }

  marketplaceCredentialDelete(request: MarketplaceCredentialRequest) {
    return this.invoke<MarketplaceCredentialStatus[]>(
      TAURI_COMMANDS.marketplaceCredentialDelete,
      { request },
    );
  }

  openExternalUrl(url: string) {
    return this.invoke<void>(TAURI_COMMANDS.openExternalUrl, { url });
  }

  async browserTabOpen(
    request: BrowserTabOpenRequest,
    observer: StreamObserver<BrowserTabEvent>,
  ) {
    const { Channel, invoke } = await import("@tauri-apps/api/core");
    const onEvent = new Channel<BrowserTabEvent>();
    onEvent.onmessage = observer;
    return invoke<BrowserTabState>(TAURI_COMMANDS.browserTabOpen, {
      request,
      [TAURI_STREAM_CHANNEL_ARGUMENT]: onEvent,
    });
  }

  browserTabNavigate(tab_id: string, url: string) {
    return this.invoke<void>(TAURI_COMMANDS.browserTabNavigate, {
      request: { tab_id, url },
    });
  }

  browserTabHistory(tab_id: string, direction: BrowserHistoryDirection) {
    return this.invoke<void>(TAURI_COMMANDS.browserTabHistory, {
      request: { tab_id, direction },
    });
  }

  browserTabReload(tab_id: string) {
    return this.invoke<void>(TAURI_COMMANDS.browserTabReload, {
      request: { tab_id },
    });
  }

  browserTabSetBounds(request: BrowserTabBoundsRequest) {
    return this.invoke<void>(TAURI_COMMANDS.browserTabSetBounds, { request });
  }

  browserTabClose(tab_id: string) {
    return this.invoke<void>(TAURI_COMMANDS.browserTabClose, {
      request: { tab_id },
    });
  }

  browserTabsReset() {
    return this.invoke<void>(TAURI_COMMANDS.browserTabsReset);
  }

  updateStatus() {
    return this.invoke<UpdateState>(TAURI_COMMANDS.updateStatus);
  }

  updateCheck() {
    return this.invoke<UpdateState>(TAURI_COMMANDS.updateCheck);
  }

  updateInstall(request: UpdateInstallRequest) {
    return this.invoke<UpdateInstallResult>(TAURI_COMMANDS.updateInstall, {
      request,
    });
  }

  guruList() {
    return this.invoke<GuruSummary[]>(TAURI_COMMANDS.guruList);
  }

  guruSelect(guru_id: string) {
    return this.invoke<GuruWorkspace>(TAURI_COMMANDS.guruSelect, { guru_id });
  }

  guruRecover(request: GuruRecoverRequest) {
    return this.invoke<GuruSummary>(TAURI_COMMANDS.guruRecover, { request });
  }

  guruCreate(request: GuruCreateRequest) {
    return this.invoke<GuruSummary>(TAURI_COMMANDS.guruCreate, { request });
  }

  guruImportMemory() {
    return this.invoke<GuruSummary | null>(TAURI_COMMANDS.guruImportMemory);
  }

  guruExportMemory(guru_id: string) {
    return this.invoke<GuruExportReceipt | null>(TAURI_COMMANDS.guruExportMemory, { guru_id });
  }

  guruRename(request: GuruRenameRequest) {
    return this.invoke<GuruSummary>(TAURI_COMMANDS.guruRename, { request });
  }

  guruDelete(request: GuruDeleteRequest) {
    return this.invoke<void>(TAURI_COMMANDS.guruDelete, { request });
  }

  chatCreate(request: ChatCreateRequest) {
    return this.invoke<ChatThread>(TAURI_COMMANDS.chatCreate, { request });
  }

  chatRename(request: ChatRenameRequest) {
    return this.invoke<ChatThread>(TAURI_COMMANDS.chatRename, { request });
  }

  chatDelete(request: ChatDeleteRequest) {
    return this.invoke<void>(TAURI_COMMANDS.chatDelete, { request });
  }

  chatAttachmentRead(
    guru_id: string,
    thread_id: string,
    message_id: string,
    attachment_id: string,
  ) {
    return this.invoke<{ data_url: string }>(TAURI_COMMANDS.chatAttachmentRead, {
      request: { guru_id, thread_id, message_id, attachment_id },
    });
  }

  chatArtifactList(guru_id: string, thread_id: string) {
    return this.invoke<ChatArtifact[]>(TAURI_COMMANDS.chatArtifactList, {
      request: { guru_id, thread_id },
    });
  }

  chatArtifactRead(
    guru_id: string,
    thread_id: string,
    artifact_id: string,
  ) {
    return this.invoke<ChatArtifactView>(TAURI_COMMANDS.chatArtifactRead, {
      request: { guru_id, thread_id, artifact_id },
    });
  }

  async chatSend(
    request: ChatSendRequest,
    observer: StreamObserver<ChatStreamEvent>,
  ) {
    const { Channel, invoke } = await import("@tauri-apps/api/core");
    const onEvent = new Channel<ChatStreamEvent>();
    onEvent.onmessage = observer;
    return invoke<{ run_id: string }>(TAURI_COMMANDS.chatSend, {
      request,
      [TAURI_STREAM_CHANNEL_ARGUMENT]: onEvent,
    });
  }

  chatSteer(request: ChatControlRequest) {
    return this.invoke<ChatControlReceipt>(TAURI_COMMANDS.chatSteer, { request });
  }

  chatAbort(run_id: string) {
    return this.invoke<void>(TAURI_COMMANDS.chatAbort, { run_id });
  }

  runActivityList() {
    return this.invoke<RunActivity[]>(TAURI_COMMANDS.runActivityList);
  }

  librarySearch(request: LibrarySearchRequest) {
    return this.invoke<LibrarySummary[]>(TAURI_COMMANDS.librarySearch, { request });
  }

  libraryRead(guru_id: string, record_id: string) {
    return this.invoke<LibraryRecord>(TAURI_COMMANDS.libraryRead, {
      guru_id,
      record_id,
    });
  }

  libraryMemoryCreate(request: LibraryMemoryCreateRequest) {
    return this.invoke<LibraryMemoryMutation>(TAURI_COMMANDS.libraryMemoryCreate, { request });
  }

  libraryMemoryUpdate(request: LibraryMemoryUpdateRequest) {
    return this.invoke<LibraryMemoryMutation>(TAURI_COMMANDS.libraryMemoryUpdate, { request });
  }

  libraryMemoryDelete(request: LibraryMemoryDeleteRequest) {
    return this.invoke<LibraryMemoryMutation>(TAURI_COMMANDS.libraryMemoryDelete, { request });
  }

  libraryMemoryRevert(request: LibraryMemoryRevertRequest) {
    return this.invoke<LibraryMemoryMutation>(TAURI_COMMANDS.libraryMemoryRevert, { request });
  }

}
