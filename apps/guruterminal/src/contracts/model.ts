export type ExecutionModelLock = {
  profile_id: string;
  name: string;
  provider: string;
  model: string;
  thinking_level: string;
  run_options: Record<string, string>;
};

export type ModelRunSelection = {
  model_profile_id: string;
  thinking_level: string;
  run_options: Record<string, string>;
};

export type ModelRunControlChoice = {
  id: string;
  label: string;
  description: string;
};

export type ModelRunControl = {
  id: string;
  label: string;
  default_choice: string;
  choices: ModelRunControlChoice[];
};

export type AgentHarnessSnapshot = {
  schema: string;
  mode: "chat";
  skill_ids: string[];
  capability_ids: string[];
  digest: string;
};

export type ProviderOauthOption = {
  label: string;
};

export type ModelProviderOption = {
  id: string;
  label: string;
  credential_label: string;
  description: string;
  api_key: boolean;
  oauth: ProviderOauthOption | null;
  credential_source: "saved" | "environment" | "missing";
  recommended: boolean;
};

export type ProviderModelOption = {
  id: string;
  name: string;
  reasoning: boolean;
  context_window: number;
  max_tokens: number;
  input: string[];
  thinking_levels: string[];
  thinking_level_map: Record<string, string | null>;
  run_controls: ModelRunControl[];
};

export type ProviderConnectionEvent =
  | { type: "opening_browser"; message: string }
  | { type: "waiting"; message: string }
  | { type: "connected"; message: string };

export type ConfiguredModel = {
  id: string;
  name: string;
  provider: string;
  model: string;
  input: string[];
  reasoning: boolean;
  context_window: number;
  max_tokens: number;
  thinking_levels: string[];
  thinking_level_map: Record<string, string | null>;
  run_controls: ModelRunControl[];
  credential_source: "saved" | "environment" | "missing";
};

export type ModelCatalog = {
  models: ConfiguredModel[];
  providers: ModelProviderOption[];
  hidden_model_profile_ids: string[];
};

export type ModelVisibilityUpdateRequest = {
  model_profile_id: string;
  visible_in_chat: boolean;
};

export type ProviderConfigureRequest = {
  provider: string;
  api_key?: string;
  clear_saved_key?: boolean;
};
