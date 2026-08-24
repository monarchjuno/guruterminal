import { useEffect, useState } from "react";
import {
  ArrowUpRightIcon,
  ChartNoAxesCombinedIcon,
  DownloadIcon,
  FileIcon,
  FileTextIcon,
} from "lucide-react";
import { Message, MessageContent } from "@/components/ai-elements/message";
import { errorMessage } from "../../errors";
import { compactTime } from "../../format";
import type { ChatArtifactRef, ChatAttachment, ChatMessage } from "../../types";
import { ChatProgressTimeline } from "./ChatProgressTimeline";
import { SafeMessageResponse } from "./SafeMessageResponse";

const attachmentSize = (bytes: number) =>
  bytes < 1024
    ? `${bytes} B`
    : bytes < 1024 * 1024
      ? `${Math.ceil(bytes / 1024)} KB`
      : `${(bytes / (1024 * 1024)).toFixed(1)} MB`;

function MessageAttachment({
  attachment,
  messageId,
  onRead,
}: {
  attachment: ChatAttachment;
  messageId: string;
  onRead: (messageId: string, attachmentId: string) => Promise<string>;
}) {
  const isImage = attachment.media_type.startsWith("image/");
  const [url, setUrl] = useState(attachment.url);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    if (!isImage || url || failed) return;
    let active = true;
    void onRead(messageId, attachment.id)
      .then((next) => {
        if (active) setUrl(next);
      })
      .catch(() => {
        if (active) setFailed(true);
      });
    return () => {
      active = false;
    };
  }, [attachment.id, failed, isImage, messageId, onRead, url]);

  const download = async () => {
    try {
      const source = url ?? (await onRead(messageId, attachment.id));
      const link = document.createElement("a");
      link.href = source;
      link.download = attachment.filename;
      link.click();
    } catch {
      setFailed(true);
    }
  };

  return (
    <div className="message-attachment">
      {isImage && url ? (
        <img src={url} alt={attachment.filename} />
      ) : (
        <FileIcon aria-hidden="true" />
      )}
      <span>
        <strong>{attachment.filename}</strong>
        <small>
          {attachment.media_type} · {attachmentSize(attachment.size_bytes)}
        </small>
        {failed && <small>Preview unavailable</small>}
      </span>
      <button
        type="button"
        aria-label={`Download ${attachment.filename}`}
        title={`Download ${attachment.filename}`}
        onClick={() => void download()}
      >
        <DownloadIcon />
      </button>
    </div>
  );
}

function memoryUpdateLabel(message: ChatMessage): string {
  const changes = message.memory_update?.changes ?? [];
  if (changes.some((change) => change.kind === "Wiki" || change.kind === "Lens")) {
    return "Guru learned";
  }
  if (changes.some((change) => change.kind === "Decision")) {
    return "Judgment saved";
  }
  return "Sources saved";
}

type Props = {
  message: ChatMessage;
  guruName: string;
  onOpenMemory: (recordId: string, title: string) => void;
  onOpenInLibrary: (recordId: string) => void;
  onOpenArtifact: (artifact: ChatArtifactRef) => void;
  onOpenLink: (url: string) => Promise<void>;
  onReadAttachment: (
    messageId: string,
    attachmentId: string,
  ) => Promise<string>;
  onRevertMemory?: (recordId: string, commitId: string) => Promise<void>;
};

export function ChatMessageCard({
  message,
  guruName,
  onOpenMemory,
  onOpenInLibrary,
  onOpenArtifact,
  onOpenLink,
  onReadAttachment,
  onRevertMemory,
}: Props) {
  const [revertingId, setRevertingId] = useState<string | null>(null);
  const [revertedIds, setRevertedIds] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
  const [revertError, setRevertError] = useState<string | null>(null);
  const usedWikiLens = (message.memory_refs ?? []).filter(
    (ref) =>
      ref.access === "exact_read" &&
      (ref.kind === "Wiki" || ref.kind === "Lens"),
  );
  const memoryUpdate = message.memory_update;
  const learnedChanges =
    memoryUpdate?.changes.filter(
      (change) => change.kind === "Wiki" || change.kind === "Lens",
    ) ?? [];
  const learnedPage = learnedChanges[0];
  const appliedCommitId = memoryUpdate?.commitId ?? null;
  const timelineItems = message.progress?.items ?? [];
  const hasActivity = timelineItems.length > 0;
  const showPendingResponse =
    message.role === "assistant" &&
    message.status === "streaming" &&
    !message.content &&
    !hasActivity;
  const showMessageBody =
    message.role === "user" || !!message.content || showPendingResponse;
  const executionModelTitle = message.execution_model
    ? [
        message.execution_model.provider,
        message.execution_model.model,
        `thinking ${message.execution_model.thinking_level}`,
      ].join(" · ")
    : undefined;

  return (
    <article className={`message ${message.role} ${message.status ?? ""}`}>
      <div className="message-meta">
        <span>{message.role === "user" ? "You" : guruName}</span>
        <time>{compactTime(message.created_at)}</time>
        {message.status === "aborted" && <span>Stopped</span>}
        {message.execution_model && (
          <span title={executionModelTitle}>
            {message.execution_model.name} ·{" "}
            {message.execution_model.thinking_level}
          </span>
        )}
      </div>
      {message.role === "assistant" && timelineItems.length > 0 && (
        <ChatProgressTimeline
          progress={
            message.progress
              ? message.progress
              : {
                  startedAtMs: Date.parse(message.created_at),
                  items: [],
                }
          }
          status={message.status}
          onOpenLink={onOpenLink}
        />
      )}
      {showMessageBody && (
        <Message from={message.role} className="max-w-full">
          <MessageContent className="message-content overflow-visible">
            {!!message.content &&
              (message.role === "user" ? (
                <p className="message-user-text">{message.content}</p>
              ) : (
                <SafeMessageResponse
                  text={message.content}
                  isAnimating={message.status === "streaming"}
                  onOpenLink={onOpenLink}
                />
              ))}
            {showPendingResponse && (
              <span className="message-response-pending">
                Starting response…
              </span>
            )}
          </MessageContent>
        </Message>
      )}

      {!!message.attachments?.length && (
        <div className="message-attachments" aria-label="Message attachments">
          {message.attachments.map((attachment) => (
            <MessageAttachment
              key={attachment.id}
              attachment={attachment}
              messageId={message.id}
              onRead={onReadAttachment}
            />
          ))}
        </div>
      )}

      {usedWikiLens.length > 0 && (
        <div className="memory-used-footer" aria-label="Used in this answer">
          <span>Used in this answer</span>
          <div className="memory-used-titles">
            {usedWikiLens.map((ref) => (
              <button
                type="button"
                key={ref.record_id}
                aria-label={`Used note: ${ref.title}`}
                onClick={() => onOpenMemory(ref.record_id, ref.title)}
              >
                {ref.title}
              </button>
            ))}
          </div>
        </div>
      )}

      {memoryUpdate?.status === "applied" && (
        <details className="memory-update-footer applied">
          <summary>{memoryUpdateLabel(message)}</summary>
          <ul className="memory-update-changes">
            {memoryUpdate.changes.map((change) => (
              <li className="memory-update-change" key={change.recordId}>
                <button
                  type="button"
                  onClick={() => onOpenMemory(change.recordId, change.title)}
                >
                  {change.title}
                </button>
                {(change.kind === "Wiki" || change.kind === "Lens") && (
                  <>
                    <span>{change.lesson}</span>
                    <small>Basis: {change.basis}</small>
                    <small>Next use: {change.futureUse}</small>
                    {onRevertMemory && appliedCommitId ? (
                      revertedIds.has(change.recordId) ? (
                        <small>Reverted</small>
                      ) : (
                        <button
                          type="button"
                          className="memory-update-revert"
                          disabled={revertingId === change.recordId}
                          onClick={() => {
                            const commitId = appliedCommitId;
                            setRevertError(null);
                            setRevertingId(change.recordId);
                            void onRevertMemory(change.recordId, commitId)
                              .then(() => {
                                setRevertedIds((current) => {
                                  const next = new Set(current);
                                  next.add(change.recordId);
                                  return next;
                                });
                              })
                              .catch((cause: unknown) => {
                                setRevertError(
                                  errorMessage(
                                    cause,
                                    "Could not revert this memory.",
                                  ),
                                );
                              })
                              .finally(() => {
                                setRevertingId((current) =>
                                  current === change.recordId ? null : current,
                                );
                              });
                          }}
                        >
                          {revertingId === change.recordId
                            ? "Reverting…"
                            : "Revert"}
                        </button>
                      )
                    ) : null}
                  </>
                )}
              </li>
            ))}
            {learnedPage ? (
              <li>
                <button
                  type="button"
                  onClick={() => onOpenInLibrary(learnedPage.recordId)}
                >
                  Open in Memory
                </button>
              </li>
            ) : null}
            {revertError ? (
              <li className="memory-update-revert-error" role="alert">
                {revertError}
              </li>
            ) : null}
          </ul>
        </details>
      )}

      {memoryUpdate?.status === "no_change" && (
        <p className="memory-update-footer" role="status">
          No durable lesson
        </p>
      )}

      {!!message.artifact_refs?.length && (
        <div className="message-artifacts" aria-label="Chat artifacts">
          {message.artifact_refs.map((artifact) => (
            <button
              className="message-artifact-card"
              type="button"
              key={`${artifact.artifact_id}:${artifact.revision}`}
              aria-label={`Open ${artifact.kind === "chart" ? "chart" : "document"} ${artifact.title}`}
              onClick={() => onOpenArtifact(artifact)}
            >
              <span className="message-artifact-icon">
                {artifact.kind === "chart" ? (
                  <ChartNoAxesCombinedIcon />
                ) : (
                  <FileTextIcon />
                )}
              </span>
              <span className="message-artifact-copy">
                <strong>{artifact.title}</strong>
                <small>
                  {artifact.kind === "chart" ? "Chart" : "Document"}
                </small>
              </span>
              <span className="message-artifact-action">
                View <ArrowUpRightIcon />
              </span>
            </button>
          ))}
        </div>
      )}
    </article>
  );
}
