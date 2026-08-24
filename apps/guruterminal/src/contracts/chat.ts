import type { GuruSummary } from "./guru";
import type { MemoryRef, MemoryUpdateResult } from "./memory";
import type { AgentHarnessSnapshot, ExecutionModelLock } from "./model";

export type ChatProgressStatus =
  | "running"
  | "succeeded"
  | "failed"
  | "stopped";

export type ChatProgressCategory =
  | "web"
  | "memory"
  | "capability"
  | "finance"
  | "files"
  | "artifact"
  | "compute"
  | "decision"
  | "system";

export type ChatProgressOperation =
  | "search"
  | "read"
  | "write"
  | "edit"
  | "list"
  | "calculate"
  | "publish"
  | "execute"
  | "submit"
  | "retry"
  | "compact"
  | "generic";

export type ChatProgressItem =
  | { id: string; kind: "commentary"; text: string }
  | {
      id: string;
      kind: "tool" | "system";
      category: ChatProgressCategory;
      operation: ChatProgressOperation;
      action: string;
      target?: string;
      href?: string;
      status: ChatProgressStatus;
      startedAtMs?: number;
      finishedAtMs?: number;
    };

export type ChatProgress = {
  startedAtMs: number;
  finishedAtMs?: number;
  items: ChatProgressItem[];
};

export type ChatArtifactKind = "markdown" | "chart";

export type ChatArtifactRef = {
  artifact_id: string;
  revision: number;
  kind: ChatArtifactKind;
  title: string;
  digest: string;
};

export type ChatArtifact = {
  id: string;
  chat_session_id: string;
  kind: ChatArtifactKind;
  title: string;
  current_revision: number;
  created_at_ms: number;
  updated_at_ms: number;
};

export type ChartColumn = {
  id: string;
  label: string;
  kind: "string" | "number" | "boolean" | "date" | "datetime";
};

export type ChartResultReceipt = {
  result_ref: string;
  runtime_id: string;
  tool_name: string;
  provider: string | null;
  request_digest: string;
  response_digest: string;
  retrieved_at: string;
  warnings: string[];
  upstream_result_refs: string[];
};

export type ChartDatasetLineage =
  | {
      kind: "from_result";
      receipt: ChartResultReceipt;
      rows_pointer: string;
      columns: Array<ChartColumn & { pointer: string }>;
    }
  | {
      kind: "agent_authored";
      upstream_receipts: ChartResultReceipt[];
    };

export type ChartDataset = {
  id: string;
  columns: ChartColumn[];
  rows: unknown[][];
  lineage: ChartDatasetLineage;
  digest: string;
};

export type ChartStudy = {
  module_id:
    | "AVP"
    | "AO"
    | "BIAS"
    | "BOLL"
    | "BRAR"
    | "BBI"
    | "CCI"
    | "CR"
    | "DMA"
    | "DMI"
    | "EMV"
    | "EMA"
    | "MTM"
    | "MA"
    | "MACD"
    | "OBV"
    | "PVT"
    | "PSY"
    | "ROC"
    | "RSI"
    | "SMA"
    | "KDJ"
    | "SAR"
    | "TRIX"
    | "VOL"
    | "VR"
    | "WR";
  calc_params: number[];
};

export type ChartDrawing = {
  kind:
    | "segment"
    | "ray"
    | "line"
    | "horizontal_line"
    | "vertical_line"
    | "price_line"
    | "fibonacci"
    | "horizontal_segment"
    | "horizontal_ray"
    | "vertical_segment"
    | "vertical_ray"
    | "parallel_line"
    | "price_channel"
    | "annotation"
    | "rectangle"
    | "arrow"
    | "measure"
    | "fibonacci_extension"
    | "long_position"
    | "short_position";
  points: Array<{ timestamp: string | number; value: number }>;
  color?: string;
  line_width?: number;
  line_style?: "solid" | "dashed";
  label?: string;
};

export type ChartDocument = {
  dataset_id: string;
  dataset_digest: string;
  view:
    | {
        kind: "financial";
        symbol: string;
        interval: string;
        time: string;
        open: string;
        high: string;
        low: string;
        close: string;
        volume?: string;
        turnover?: string;
        price_precision?: number;
      }
    | {
        kind: "analytic";
        chart_type: "line" | "area" | "bar" | "scatter";
        x: string;
        y: string[];
        color?: string;
        semantic_types: Record<string, string>;
        title?: string;
        subtitle?: string;
      };
  studies: ChartStudy[];
  drawings: ChartDrawing[];
  note?: string;
};

export type ChatArtifactPayload =
  | {
      kind: "markdown";
      schema: "guruterminal-markdown/1";
      markdown: string;
    }
  | {
      kind: "chart";
      schema: "guruterminal-chart/2";
      chart: ChartDocument;
    };

export type ChatArtifactRevision = {
  artifact_id: string;
  revision: number;
  payload: ChatArtifactPayload;
  digest: string;
  source_message_id: string;
  created_at_ms: number;
};

export type ChatArtifactView = {
  artifact: ChatArtifact;
  revision: ChatArtifactRevision;
  chart_dataset?: ChartDataset;
};

export type ChatMessage = {
  id: string;
  role: "user" | "assistant";
  content: string;
  created_at: string;
  status?: "streaming" | "complete" | "aborted" | "error";
  memory_refs?: MemoryRef[];
  observed_exact_count?: number;
  refs_truncated?: boolean;
  refs_digest?: string;
  memory_update?: MemoryUpdateResult;
  memory_revision?: string;
  execution_model?: ExecutionModelLock;
  agent_harness?: AgentHarnessSnapshot;
  decision?: ChatDecision;
  progress?: ChatProgress;
  attachments?: ChatAttachment[];
  artifact_refs?: ChatArtifactRef[];
};

export type ChatDecision = {
  payload: {
    stance: "positive" | "neutral" | "negative" | "abstain";
    horizon: string;
    probability: number;
    thesis: string;
    evidence_ids: string[];
    risks: string[];
    invalidation_conditions: string[];
  };
  digest: string;
  sealed_at_ms: number;
};

export type ChatAttachment = {
  id: string;
  filename: string;
  media_type: string;
  size_bytes: number;
  url?: string;
};

export type ChatAttachmentInput = {
  filename: string;
  media_type: string;
  data_base64: string;
};

export type ChatThread = {
  id: string;
  guru_id: string;
  title: string;
  updated_at: string;
  use_memory: boolean;
  update_memory: boolean;
  messages: ChatMessage[];
};

export type GuruWorkspace = {
  guru: GuruSummary;
  threads: ChatThread[];
};

export type ChatCreateRequest = {
  guru_id: string;
  title?: string;
};

export type ChatRenameRequest = {
  guru_id: string;
  thread_id: string;
  title: string;
};

export type ChatDeleteRequest = {
  guru_id: string;
  thread_id: string;
};

export type ChatSendRequest = {
  run_id: string;
  guru_id: string;
  thread_id: string;
  prompt: string;
  use_memory: boolean;
  update_memory: boolean;
  as_of?: string;
  model_profile_id: string;
  thinking_level: string;
  run_options: Record<string, string>;
  attachments: ChatAttachmentInput[];
};

export type ChatControlRequest = {
  guru_id: string;
  thread_id: string;
  prompt: string;
};

export type ChatControlReceipt = {
  message_id: string;
  prompt: string;
  created_at: string;
  mode: "steer";
};

export type ChatStreamEvent =
  | { type: "started"; run_id: string }
  | { type: "memory"; run_id: string; memories: MemoryRef[] }
  | { type: "delta"; run_id: string; text: string }
  | { type: "title"; run_id: string; title: string }
  | { type: "progress"; run_id: string; progress: ChatProgress }
  | {
      type: "memory_update";
      run_id: string;
      result: MemoryUpdateResult;
    }
  | {
      type: "decision";
      run_id: string;
      decision: ChatDecision;
    }
  | {
      type: "artifact";
      run_id: string;
      artifact: ChatArtifactRef;
    }
  | {
      type: "completed";
      run_id: string;
      message_id: string;
      final_text: string;
      created_at: string;
      execution_model: ExecutionModelLock;
      agent_harness: AgentHarnessSnapshot;
    }
  | { type: "aborted"; run_id: string }
  | { type: "error"; run_id: string; message: string };
