export type UpdatePhase =
  | "idle"
  | "checking"
  | "confirming"
  | "downloading"
  | "installing"
  | "restarting";

export type UpdateOffer = {
  offer_id: string;
  version: string;
  notes: string | null;
  published_at: string | null;
};

export type UpdateBlocker = {
  id: string;
  kind: string;
  label: string;
};

export type UpdateState = {
  supported: boolean;
  current_version: string;
  phase: UpdatePhase;
  offer: UpdateOffer | null;
  downloaded_bytes: number;
  total_bytes: number | null;
  last_checked_at_ms: number | null;
  next_auto_check_at_ms: number | null;
  error: string | null;
  blockers: UpdateBlocker[];
};

export type UpdateInstallRequest = {
  offer_id: string;
};

export type UpdateInstallResult = {
  outcome: "blocked" | "cancelled";
  blockers: UpdateBlocker[];
};

export type StreamObserver<T> = (event: T) => void;

export type RunActivity = {
  run_id: string;
  guru_id: string;
  kind:
    | "chat"
    | "memory_write"
    | "chat_mutation"
    | "memory_mutation";
  target: string;
  started_at_ms: number;
};
