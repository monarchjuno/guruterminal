import type { ChatTransport, FileUIPart, UIMessage, UIMessageChunk } from "ai";
import type {
  AgentHarnessSnapshot,
  ChatAttachment,
  ChatArtifactRef,
  ChatDecision,
  ChatMessage,
  ChatPerformance,
  ChatProgress,
  ChatProgressPatch,
  ChatStreamEvent,
  GuruTerminalBridge,
  MemoryRef,
  MemoryUpdateResult,
} from "../types";
import { errorMessage } from "../errors";

export type GuruChatMetadata = {
  native_message_id?: string;
  created_at?: string;
  status?: ChatMessage["status"];
  memory_revision?: string;
  observed_exact_count?: number;
  refs_truncated?: boolean;
  refs_digest?: string;
  execution_model?: import("../types").ExecutionModelLock;
  agent_harness?: AgentHarnessSnapshot;
  final_text?: string;
  progress?: ChatProgress;
  performance?: ChatPerformance;
};

export type GuruChatData = {
  progress: ChatProgress;
  attachment: ChatAttachment;
  memory: MemoryRef[];
  memory_update: MemoryUpdateResult;
  decision: ChatDecision;
  artifact: ChatArtifactRef;
  run: { run_id: string };
  performance: ChatPerformance;
};

export type GuruUIMessage = UIMessage<GuruChatMetadata, GuruChatData>;

export type TauriChatRequestContext = {
  guru_id: string;
  thread_id: string;
  use_memory: boolean;
  update_memory: boolean;
  model_profile_id: string;
  thinking_level: string;
  run_options: Record<string, string>;
};

type ChatTitleHandler = (
  guruId: string,
  threadId: string,
  title: string,
) => void;
type ChatArtifactHandler = (
  guruId: string,
  threadId: string,
  artifact: ChatArtifactRef,
) => void;

const messageText = (message: GuruUIMessage) =>
  message.parts
    .filter((part) => part.type === "text")
    .map((part) => part.text)
    .join("");

const activeDraftText = (message: GuruUIMessage) => {
  for (let index = message.parts.length - 1; index >= 0; index -= 1) {
    const part = message.parts[index];
    if (part.type === "reasoning" && part.state === "streaming") {
      return part.text;
    }
  }
  return "";
};

const UNKNOWN_MESSAGE_CREATED_AT = "1970-01-01T00:00:00.000Z";

export type ChatProgressStreamState = {
  progress: ChatProgress;
  sequence: number;
};

export const applyChatProgressSnapshot = (
  state: ChatProgressStreamState | undefined,
  progress: ChatProgress,
  sequence = 0,
): ChatProgressStreamState =>
  state && sequence <= state.sequence ? state : { progress, sequence };

export const applyChatProgressPatch = (
  state: ChatProgressStreamState | undefined,
  patch: ChatProgressPatch,
): ChatProgressStreamState | undefined => {
  if (!state || patch.sequence !== state.sequence + 1) return state;

  const removeIds = new Set(patch.removeItemIds ?? []);
  const items = state.progress.items
    .filter((item) => !removeIds.has(item.id))
    .map((item) => ({ ...item }));
  const indices = new Map(items.map((item, index) => [item.id, index]));
  for (const item of patch.upsertItems ?? []) {
    const index = indices.get(item.id);
    if (index === undefined) {
      indices.set(item.id, items.length);
      items.push(item);
    } else {
      items[index] = item;
    }
  }
  return {
    sequence: patch.sequence,
    progress: {
      ...state.progress,
      ...(patch.finishedAtMs === undefined
        ? {}
        : { finishedAtMs: patch.finishedAtMs }),
      items,
    },
  };
};

const dataUrlPayload = (file: FileUIPart) => {
  const match = file.url.match(/^data:([^;,]+);base64,([A-Za-z0-9+/=]+)$/);
  if (!match) throw new Error("Attachment data is unavailable.");
  return {
    filename: file.filename?.trim() || "attachment",
    media_type: file.mediaType || match[1],
    data_base64: match[2],
  };
};

const estimatedBase64Bytes = (url: string) => {
  const payload = url.split(",", 2)[1] ?? "";
  return Math.max(1, Math.floor((payload.replace(/=+$/u, "").length * 3) / 4));
};

export const toGuruUIMessage = (message: ChatMessage): GuruUIMessage => {
  return {
    id: message.id,
    role: message.role,
    metadata: {
      native_message_id: message.id,
      created_at: message.created_at,
      status: message.status ?? "complete",
      memory_revision: message.memory_revision,
      observed_exact_count: message.observed_exact_count,
      refs_truncated: message.refs_truncated,
      refs_digest: message.refs_digest,
      execution_model: message.execution_model,
      agent_harness: message.agent_harness,
      performance: message.performance,
    },
    parts: [
      { type: "text", text: message.content, state: "done" },
      ...(message.attachments?.map((attachment) => ({
        type: "data-attachment" as const,
        id: attachment.id,
        data: attachment,
      })) ?? []),
      ...(message.memory_refs
        ? ([{ type: "data-memory", data: message.memory_refs }] as const)
        : []),
      ...(message.memory_update
        ? ([
            { type: "data-memory_update", data: message.memory_update },
          ] as const)
        : []),
      ...(message.decision
        ? ([{ type: "data-decision", data: message.decision }] as const)
        : []),
      ...(message.artifact_refs?.map((artifact) => ({
        type: "data-artifact" as const,
        id: `${artifact.artifact_id}:${artifact.revision}`,
        data: artifact,
      })) ?? []),
      ...(message.progress
        ? ([
            { type: "data-progress", id: "progress", data: message.progress },
          ] as const)
        : []),
    ],
  };
};

export const fromGuruUIMessage = (message: GuruUIMessage): ChatMessage => {
  const status = message.metadata?.status ?? "complete";
  const memoryPart = message.parts.find((part) => part.type === "data-memory");
  const memoryUpdatePart = message.parts.find(
    (part) => part.type === "data-memory_update",
  );
  const decisionPart = message.parts.find(
    (part) => part.type === "data-decision",
  );
  const progressPart = message.parts.find(
    (part) => part.type === "data-progress",
  );
  const progress = message.metadata?.progress ?? progressPart?.data;
  const artifactRefs = message.parts
    .filter((part) => part.type === "data-artifact")
    .map((part) => part.data);
  const attachmentParts = message.parts
    .filter((part) => part.type === "data-attachment")
    .map((part) => part.data);
  const fileAttachments = message.parts
    .filter((part) => part.type === "file")
    .map((part, index) => ({
      id: `${message.id}-attachment-${index + 1}`,
      filename: part.filename ?? `attachment-${index + 1}`,
      media_type: part.mediaType,
      size_bytes: estimatedBase64Bytes(part.url),
      url: part.url,
    }));

  return {
    id: message.metadata?.native_message_id ?? message.id,
    role: message.role === "assistant" ? "assistant" : "user",
    content:
      message.role === "assistant" && message.metadata?.final_text !== undefined
        ? message.metadata.final_text
        : messageText(message) ||
          (message.role === "assistant" && status === "streaming"
            ? activeDraftText(message)
            : ""),
    created_at: message.metadata?.created_at ?? UNKNOWN_MESSAGE_CREATED_AT,
    status,
    memory_refs: memoryPart?.data,
    memory_update: memoryUpdatePart?.data,
    decision: decisionPart?.data,
    memory_revision: message.metadata?.memory_revision,
    observed_exact_count: message.metadata?.observed_exact_count,
    refs_truncated: message.metadata?.refs_truncated,
    refs_digest: message.metadata?.refs_digest,
    execution_model: message.metadata?.execution_model,
    agent_harness: message.metadata?.agent_harness,
    performance: message.metadata?.performance,
    progress,
    attachments:
      attachmentParts.length > 0
        ? attachmentParts
        : fileAttachments.length > 0
          ? fileAttachments
          : undefined,
    artifact_refs: artifactRefs.length ? artifactRefs : undefined,
  };
};

export class TauriChatTransport implements ChatTransport<GuruUIMessage> {
  constructor(
    private readonly bridge: GuruTerminalBridge,
    private readonly onTitle?: ChatTitleHandler,
    private readonly onArtifact?: ChatArtifactHandler,
  ) {}

  async sendMessages({
    messages,
    abortSignal,
    body,
  }: Parameters<ChatTransport<GuruUIMessage>["sendMessages"]>[0]) {
    const context = body as TauriChatRequestContext | undefined;
    if (!context?.guru_id || !context.thread_id) {
      throw new Error("Guru and thread are required for a native chat run.");
    }

    const lastUserMessage = [...messages]
      .reverse()
      .find((message) => message.role === "user");
    const prompt = lastUserMessage ? messageText(lastUserMessage).trim() : "";
    const attachments = lastUserMessage
      ? lastUserMessage.parts
          .filter((part) => part.type === "file")
          .map(dataUrlPayload)
      : [];
    if (!prompt && attachments.length === 0) {
      throw new Error("A message or attachment is required.");
    }

    return new ReadableStream<UIMessageChunk>({
      start: (controller) => {
        const responseId = `assistant-${crypto.randomUUID()}`;
        const textId = `text-${responseId}`;
        const runId = `chat-ui-${crypto.randomUUID()}`;
        let started = false;
        let finished = false;
        let hasText = false;
        let textStarted = false;
        let activeDraftId: string | undefined;
        let bufferedDraftId: string | undefined;
        let bufferedDraftDelta = "";
        let draftFlushTimer: number | undefined;
        let abortRequested = false;
        let abortInFlight = false;
        let abortConfirmed = false;
        let abortAttempts = 0;
        let abortRetry: number | undefined;
        let progressState: ChatProgressStreamState | undefined;

        const enqueue = (chunk: UIMessageChunk) => {
          if (!finished && !abortSignal?.aborted) controller.enqueue(chunk);
        };
        const startResponse = (createdAt = new Date().toISOString()) => {
          if (started) return;
          started = true;
          enqueue({ type: "start", messageId: responseId });
          enqueue({
            type: "message-metadata",
            messageMetadata: { created_at: createdAt, status: "streaming" },
          });
        };
        const startText = () => {
          if (textStarted) return;
          textStarted = true;
          enqueue({ type: "text-start", id: textId });
        };
        const flushDraftDelta = () => {
          if (draftFlushTimer !== undefined) {
            window.clearTimeout(draftFlushTimer);
            draftFlushTimer = undefined;
          }
          if (!bufferedDraftId || !bufferedDraftDelta) return;
          const id = bufferedDraftId;
          const delta = bufferedDraftDelta;
          bufferedDraftId = undefined;
          bufferedDraftDelta = "";
          enqueue({ type: "reasoning-delta", id, delta });
        };
        const bufferDraftDelta = (id: string, delta: string) => {
          if (bufferedDraftId && bufferedDraftId !== id) flushDraftDelta();
          bufferedDraftId = id;
          bufferedDraftDelta += delta;
          if (draftFlushTimer === undefined) {
            draftFlushTimer = window.setTimeout(flushDraftDelta, 32);
          }
        };
        const finishDraft = () => {
          flushDraftDelta();
          if (!activeDraftId) return;
          enqueue({ type: "reasoning-end", id: activeDraftId });
          activeDraftId = undefined;
        };
        const close = () => {
          if (finished) return;
          finished = true;
          if (abortRetry !== undefined) window.clearTimeout(abortRetry);
          if (draftFlushTimer !== undefined) {
            window.clearTimeout(draftFlushTimer);
          }
          if (!abortSignal?.aborted) controller.close();
          abortSignal?.removeEventListener("abort", abortNativeRun);
        };
        const fail = (
          message: string,
          terminal?: Extract<ChatStreamEvent, { type: "error" }>,
        ) => {
          startResponse(terminal?.created_at);
          finishDraft();
          const finalText = terminal?.final_text ?? message;
          if (!hasText) {
            hasText = true;
            startText();
            enqueue({ type: "text-delta", id: textId, delta: finalText });
          }
          if (terminal?.progress) {
            enqueue({
              type: "data-progress",
              id: `progress-${responseId}`,
              data: terminal.progress,
            });
          }
          enqueue({
            type: "message-metadata",
            messageMetadata: {
              status: "error",
              ...(terminal
                ? {
                    native_message_id: terminal.message_id,
                    final_text: terminal.final_text,
                    created_at: terminal.created_at,
                    execution_model: terminal.execution_model,
                    agent_harness: terminal.agent_harness,
                  }
                : {}),
            },
          });
          if (textStarted) enqueue({ type: "text-end", id: textId });
          enqueue({ type: "error", errorText: message });
          close();
        };
        const abortNativeRun = () => {
          abortRequested = true;
          if (
            finished ||
            abortConfirmed ||
            abortInFlight ||
            abortAttempts >= 50
          ) {
            return;
          }
          abortInFlight = true;
          abortAttempts += 1;
          void this.bridge
            .chatAbort(runId)
            .then(() => {
              abortConfirmed = true;
            })
            .catch(() => {
              if (!finished && abortRequested && abortAttempts < 50) {
                abortRetry = window.setTimeout(abortNativeRun, 100);
              }
            })
            .finally(() => {
              abortInFlight = false;
            });
        };

        abortSignal?.addEventListener("abort", abortNativeRun, { once: true });

        const onEvent = (event: ChatStreamEvent) => {
          if (finished) return;
          if (event.run_id !== runId) return;
          if (event.type !== "assistant_draft_delta") flushDraftDelta();
          if (event.type === "started") {
            startResponse();
            enqueue({
              type: "data-run",
              data: { run_id: event.run_id },
              transient: true,
            });
            if (abortSignal?.aborted) abortNativeRun();
            return;
          }

          if (event.type === "title") {
            if (!abortSignal?.aborted) {
              this.onTitle?.(context.guru_id, context.thread_id, event.title);
            }
            return;
          }

          startResponse(
            event.type === "completed" || event.type === "error"
              ? event.created_at
              : undefined,
          );
          if (event.type === "memory") {
            enqueue({ type: "data-memory", data: event.memories });
            return;
          }
          if (event.type === "assistant_draft_started") {
            finishDraft();
            activeDraftId = event.draft_id;
            enqueue({ type: "reasoning-start", id: event.draft_id });
            return;
          }
          if (event.type === "assistant_draft_delta") {
            if (activeDraftId !== event.draft_id) {
              finishDraft();
              activeDraftId = event.draft_id;
              enqueue({ type: "reasoning-start", id: event.draft_id });
            }
            bufferDraftDelta(event.draft_id, event.delta);
            return;
          }
          if (event.type === "assistant_draft_finished") {
            if (activeDraftId === event.draft_id) finishDraft();
            return;
          }
          if (event.type === "generation_finished") {
            enqueue({
              type: "data-performance",
              id: `performance-${responseId}`,
              data: event.performance,
              transient: true,
            });
            return;
          }
          if (event.type === "delta") {
            hasText = true;
            startText();
            enqueue({ type: "text-delta", id: textId, delta: event.text });
            return;
          }
          if (event.type === "progress") {
            const next = applyChatProgressSnapshot(
              progressState,
              event.progress,
              event.sequence,
            );
            if (next === progressState) return;
            progressState = next;
            enqueue({
              type: "data-progress",
              id: `progress-${responseId}`,
              data: next.progress,
            });
            return;
          }
          if (event.type === "progress_patch") {
            const next = applyChatProgressPatch(progressState, event.patch);
            if (!next || next === progressState) return;
            progressState = next;
            enqueue({
              type: "data-progress",
              id: `progress-${responseId}`,
              data: next.progress,
            });
            return;
          }
          if (event.type === "memory_update") {
            enqueue({ type: "data-memory_update", data: event.result });
            return;
          }
          if (event.type === "decision") {
            enqueue({ type: "data-decision", data: event.decision });
            return;
          }
          if (event.type === "artifact") {
            enqueue({
              type: "data-artifact",
              id: `${event.artifact.artifact_id}:${event.artifact.revision}`,
              data: event.artifact,
            });
            if (!abortSignal?.aborted) {
              this.onArtifact?.(
                context.guru_id,
                context.thread_id,
                event.artifact,
              );
            }
            return;
          }
          if (event.type === "completed") {
            finishDraft();
            if (!hasText && event.final_text) {
              hasText = true;
              startText();
              enqueue({
                type: "text-delta",
                id: textId,
                delta: event.final_text,
              });
            }
            enqueue({
              type: "message-metadata",
              messageMetadata: {
                native_message_id: event.message_id,
                final_text: event.final_text,
                created_at: event.created_at,
                status: "complete",
                execution_model: event.execution_model,
                agent_harness: event.agent_harness,
                performance: event.performance,
              },
            });
            if (textStarted) enqueue({ type: "text-end", id: textId });
            enqueue({ type: "finish", finishReason: "stop" });
            close();
            return;
          }
          if (event.type === "aborted") {
            finishDraft();
            if (!hasText) {
              hasText = true;
              startText();
              enqueue({
                type: "text-delta",
                id: textId,
                delta: "Response stopped.",
              });
            }
            enqueue({
              type: "message-metadata",
              messageMetadata: {
                status: "aborted",
              },
            });
            if (textStarted) enqueue({ type: "text-end", id: textId });
            enqueue({ type: "abort", reason: "Stopped by user" });
            close();
            return;
          }
          fail(event.message, event);
        };

        void this.bridge
          .chatSend(
            {
              ...context,
              run_id: runId,
              prompt,
              attachments,
            },
            onEvent,
          )
          .then(({ run_id: acknowledgedRunId }) => {
            if (acknowledgedRunId !== runId) {
              fail("The native worker acknowledged a different Chat run.");
              return;
            }
            if (abortSignal?.aborted) abortNativeRun();
          })
          .catch((cause) => {
            if (finished) return;
            fail(errorMessage(cause, "Could not start the response."));
          });
      },
    });
  }

  async reconnectToStream() {
    return null;
  }
}
