export type GuruRecoveryAction = "recover_memory";

export type GuruAvailability =
  | { status: "available" }
  | {
      status: "recovery_required";
      reason: "interrupted_memory_update";
      action: GuruRecoveryAction;
    };

export type GuruSummary = {
  id: string;
  name: string;
  philosophy: string;
  record_count: number;
  updated_at: string;
  accent: string;
  enabled_skill_ids: string[];
  last_model_profile_id?: string;
  availability: GuruAvailability;
};

export type GuruCreateRequest = { name: string };
export type GuruRenameRequest = { guru_id: string; name: string };
export type GuruDeleteRequest = { guru_id: string };
export type GuruRecoverRequest = {
  guru_id: string;
  action: GuruRecoveryAction;
};

export type GuruExportReceipt = {
  guru_id: string;
  record_count: number;
  memory_revision: string;
};

export type AgentSkillSummary = {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  ownership: "bundled" | "user";
  editable: boolean;
  current_revision_id?: string;
};

export type AgentSkillsUpdateRequest = {
  guru_id: string;
  skill_ids: string[];
};

export type GuruCapabilityRequest = { guru_id: string; entry_id: string };

export type MarketplaceConnectorConfigureRequest = {
  entry_id: string;
  config: Record<string, string>;
};

export type MarketplaceCredentialRequest = {
  entry_id: string;
};

export type MarketplaceCredentialSaveRequest = {
  entry_id: string;
  secrets: Record<string, string>;
};
