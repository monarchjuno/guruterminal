import type {
  GuruCapabilityBinding,
  MarketplaceCredentialStatus,
  MarketplaceSnapshot,
} from "../marketplace/types";
import type {
  BrowserHistoryDirection,
  BrowserTabBoundsRequest,
  BrowserTabEvent,
  BrowserTabOpenRequest,
  BrowserTabState,
} from "./browser";
import type {
  ChatArtifact,
  ChatArtifactView,
  ChatControlReceipt,
  ChatControlRequest,
  ChatCreateRequest,
  ChatDeleteRequest,
  ChatRenameRequest,
  ChatSendRequest,
  ChatStreamEvent,
  ChatThread,
  GuruWorkspace,
} from "./chat";
import type {
  AgentSkillSummary,
  AgentSkillsUpdateRequest,
  GuruCapabilityRequest,
  GuruCreateRequest,
  GuruDeleteRequest,
  GuruExportReceipt,
  GuruRecoverRequest,
  GuruRenameRequest,
  GuruSummary,
  MarketplaceCredentialRequest,
  MarketplaceCredentialSaveRequest,
  MarketplaceConnectorConfigureRequest,
} from "./guru";
import type {
  LibraryRecord,
  LibraryMemoryCreateRequest,
  LibraryMemoryDeleteRequest,
  LibraryMemoryMutation,
  LibraryMemoryRevertRequest,
  LibraryMemoryUpdateRequest,
  LibrarySearchRequest,
  LibrarySummary,
} from "./memory";
import type {
  ModelCatalog,
  ModelVisibilityUpdateRequest,
  ProviderConfigureRequest,
  ProviderConnectionEvent,
} from "./model";
import type {
  RunActivity,
  StreamObserver,
  UpdateInstallRequest,
  UpdateInstallResult,
  UpdateState,
} from "./runtime";

export interface GuruTerminalBridge {
  modelCatalogGet(): Promise<ModelCatalog>;
  modelVisibilityUpdate(
    request: ModelVisibilityUpdateRequest,
  ): Promise<ModelCatalog>;
  providerModels(provider: string): Promise<ModelCatalog>;
  providerConfigure(request: ProviderConfigureRequest): Promise<ModelCatalog>;
  providerConnect(
    provider: string,
    observer: StreamObserver<ProviderConnectionEvent>,
  ): Promise<ModelCatalog>;
  providerConnectCancel(): Promise<void>;
  providerConnectOpenBrowser(): Promise<void>;
  providerDisconnect(provider: string): Promise<ModelCatalog>;
  marketplaceSnapshot(): Promise<MarketplaceSnapshot>;
  guruCapabilityList(guru_id: string): Promise<GuruCapabilityBinding[]>;
  agentSkillCatalog(guru_id: string): Promise<AgentSkillSummary[]>;
  agentSkillsUpdate(request: AgentSkillsUpdateRequest): Promise<GuruSummary>;
  guruCapabilityEnable(
    request: GuruCapabilityRequest,
  ): Promise<GuruCapabilityBinding>;
  marketplaceConnectorConfigure(
    request: MarketplaceConnectorConfigureRequest,
  ): Promise<void>;
  guruCapabilityDisable(
    request: GuruCapabilityRequest,
  ): Promise<GuruCapabilityBinding>;
  marketplaceCredentialSave(
    request: MarketplaceCredentialSaveRequest,
  ): Promise<MarketplaceCredentialStatus[]>;
  marketplaceCredentialVerify(
    request: MarketplaceCredentialRequest,
  ): Promise<MarketplaceCredentialStatus[]>;
  marketplaceCredentialDelete(
    request: MarketplaceCredentialRequest,
  ): Promise<MarketplaceCredentialStatus[]>;
  openExternalUrl(url: string): Promise<void>;
  browserTabOpen(
    request: BrowserTabOpenRequest,
    observer: StreamObserver<BrowserTabEvent>,
  ): Promise<BrowserTabState>;
  browserTabNavigate(tab_id: string, url: string): Promise<void>;
  browserTabHistory(
    tab_id: string,
    direction: BrowserHistoryDirection,
  ): Promise<void>;
  browserTabReload(tab_id: string): Promise<void>;
  browserTabSetBounds(request: BrowserTabBoundsRequest): Promise<void>;
  browserTabClose(tab_id: string): Promise<void>;
  browserTabsReset(): Promise<void>;
  updateStatus(): Promise<UpdateState>;
  updateCheck(): Promise<UpdateState>;
  updateInstall(request: UpdateInstallRequest): Promise<UpdateInstallResult>;
  guruList(): Promise<GuruSummary[]>;
  guruSelect(guru_id: string): Promise<GuruWorkspace>;
  guruRecover(request: GuruRecoverRequest): Promise<GuruSummary>;
  guruCreate(request: GuruCreateRequest): Promise<GuruSummary>;
  guruImportMemory(): Promise<GuruSummary | null>;
  guruExportMemory(guru_id: string): Promise<GuruExportReceipt | null>;
  guruRename(request: GuruRenameRequest): Promise<GuruSummary>;
  guruDelete(request: GuruDeleteRequest): Promise<void>;
  chatCreate(request: ChatCreateRequest): Promise<ChatThread>;
  chatRename(request: ChatRenameRequest): Promise<ChatThread>;
  chatDelete(request: ChatDeleteRequest): Promise<void>;
  chatAttachmentRead(
    guru_id: string,
    thread_id: string,
    message_id: string,
    attachment_id: string,
  ): Promise<{ data_url: string }>;
  chatArtifactList(guru_id: string, thread_id: string): Promise<ChatArtifact[]>;
  chatArtifactRead(
    guru_id: string,
    thread_id: string,
    artifact_id: string,
  ): Promise<ChatArtifactView>;
  chatSend(
    request: ChatSendRequest,
    observer: StreamObserver<ChatStreamEvent>,
  ): Promise<{ run_id: string }>;
  chatSteer(request: ChatControlRequest): Promise<ChatControlReceipt>;
  chatAbort(run_id: string): Promise<void>;
  runActivityList(): Promise<RunActivity[]>;
  librarySearch(request: LibrarySearchRequest): Promise<LibrarySummary[]>;
  libraryRead(guru_id: string, record_id: string): Promise<LibraryRecord>;
  libraryMemoryCreate(request: LibraryMemoryCreateRequest): Promise<LibraryMemoryMutation>;
  libraryMemoryUpdate(request: LibraryMemoryUpdateRequest): Promise<LibraryMemoryMutation>;
  libraryMemoryDelete(request: LibraryMemoryDeleteRequest): Promise<LibraryMemoryMutation>;
  libraryMemoryRevert(request: LibraryMemoryRevertRequest): Promise<LibraryMemoryMutation>;
}
