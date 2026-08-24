export type MarketplaceEntryKind = "data_source" | "analysis_tool";
export type MarketplaceFreeState =
  "keyless" | "free_account" | "local" | "paid";
export type MarketplaceTrust = "first_party" | "reviewed_community";
export type MarketplaceReleaseStage = "available" | "preview";
export type MarketplaceSourceStatus = "ready" | "coming_soon";
export type MarketplaceInstallationPolicy =
  | "INSTALLED_BY_DEFAULT"
  | "AVAILABLE"
  | "NOT_AVAILABLE";
export type MarketplaceAuthenticationPolicy = "ON_INSTALL" | "ON_USE";

export type MarketplaceRuntime = {
  kind: "native" | "local_worker" | "mcp";
  server_id: string | null;
  worker_id: string | null;
  provider_ids: string[];
  credential_mapping: Record<string, string>;
  config_mapping: Record<string, string>;
  verification_probe: {
    tool_name: string;
    arguments: Record<string, unknown>;
  } | null;
};

export type MarketplacePermissions = {
  network_hosts: string[];
  credential_kinds: string[];
};

export type MarketplaceSetupField = {
  id: string;
  kind: "api_key" | "email" | "select";
  options: string[];
  label: string;
  required: boolean;
  min_length: number;
  max_length: number;
  help_url: string | null;
};

export type MarketplaceSetup = {
  config_fields: MarketplaceSetupField[];
  credential_fields: MarketplaceSetupField[];
  credential_scope_fields?: string[];
};

export type MarketplaceEntry = {
  id: string;
  plugin: string;
  name: string;
  summary: string;
  publisher: string;
  data_authority: string;
  kind: MarketplaceEntryKind;
  free_state: MarketplaceFreeState;
  trust: MarketplaceTrust;
  runtime: MarketplaceRuntime;
  release_stage: MarketplaceReleaseStage;
  featured: boolean;
  markets: string[];
  asset_classes: string[];
  capabilities: string[];
  freshness: string[];
  attribution: string;
  terms_url: string | null;
  permissions: MarketplacePermissions;
  setup?: MarketplaceSetup;
};

export type MarketplaceCatalog = {
  schema_version: "guruterminal-marketplace-catalog/1";
  entries: MarketplaceEntry[];
};

export type MarketplaceSource = {
  id: string;
  display_name: string;
  status: MarketplaceSourceStatus;
  summary: string;
};

export type MarketplacePlugin = {
  name: string;
  version: string;
  description: string;
  author: { name: string; email?: string; url?: string };
  interface: {
    displayName: string;
    shortDescription: string;
    category: string;
    capabilities: string[];
  };
  policy: {
    installation: MarketplaceInstallationPolicy;
    authentication?: MarketplaceAuthenticationPolicy;
  };
  category: string;
};

export type MarketplaceInstalled = {
  entry_id: string;
  configured: boolean;
  health: "ready" | "needs_configuration" | "disabled" | "error";
};

export type GuruCapabilityBinding = {
  entry_id: string;
  enabled: boolean;
  granted_permissions: string[];
  available: boolean;
};

export type MarketplaceConnectorStatus = {
  entry_id: string;
  config: Record<string, string>;
  config_state: "not_required" | "missing" | "valid";
  credentials: MarketplaceCredentialStatus[];
  readiness: "ready" | "needs_configuration" | "runtime_unavailable";
};

export type MarketplaceCredentialStatus = {
  entry_id: string;
  credential_id: string;
  stored: boolean;
  active: boolean;
  pending: boolean;
  verification: "never" | "verified" | "rejected" | "temporarily_unavailable";
  verified_at: number | null;
  last_error: string | null;
};

export type MarketplaceSnapshot = {
  schema_version: "guruterminal-marketplace-snapshot/1";
  sources: MarketplaceSource[];
  plugins: MarketplacePlugin[];
  catalog: MarketplaceCatalog;
  installed: MarketplaceInstalled[];
  connectors: MarketplaceConnectorStatus[];
};
