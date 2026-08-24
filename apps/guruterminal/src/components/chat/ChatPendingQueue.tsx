import { useState } from "react";
import {
  ArrowDownIcon,
  ArrowUpIcon,
  CheckIcon,
  PencilIcon,
  SendIcon,
  Trash2Icon,
  XIcon,
} from "lucide-react";

export type QueuedChatMessage = {
  id: string;
  text: string;
  createdAt: string;
};

type Props = {
  queued: QueuedChatMessage[];
  holdReason?: string | null;
  onRemove: (id: string) => void;
  onEdit: (id: string, text: string) => void;
  onMove: (id: string, offset: -1 | 1) => void;
  onSendNow: (id: string) => void;
  canSendNow?: boolean;
};

export function ChatPendingQueue({
  queued,
  holdReason,
  onRemove,
  onEdit,
  onMove,
  onSendNow,
  canSendNow = true,
}: Props) {
  if (queued.length === 0 && !holdReason) return null;

  return (
    <div className="chat-pending-queue" aria-label="Pending chat instructions">
      {holdReason && (
        <p className="chat-pending-hold" role="status">
          {holdReason}
        </p>
      )}
      {queued.map((item, index) => (
        <QueuedCard
          item={item}
          key={item.id}
          canMoveUp={index > 0}
          canMoveDown={index < queued.length - 1}
          onRemove={() => onRemove(item.id)}
          onEdit={(text) => onEdit(item.id, text)}
          onMoveUp={() => onMove(item.id, -1)}
          onMoveDown={() => onMove(item.id, 1)}
          onSendNow={() => onSendNow(item.id)}
          canSendNow={canSendNow}
        />
      ))}
    </div>
  );
}

function QueuedCard({
  item,
  canMoveUp,
  canMoveDown,
  onRemove,
  onEdit,
  onMoveUp,
  onMoveDown,
  onSendNow,
  canSendNow,
}: {
  item: QueuedChatMessage;
  canMoveUp: boolean;
  canMoveDown: boolean;
  onRemove: () => void;
  onEdit: (text: string) => void;
  onMoveUp: () => void;
  onMoveDown: () => void;
  onSendNow: () => void;
  canSendNow: boolean;
}) {
  const [draft, setDraft] = useState(item.text);
  const [editing, setEditing] = useState(false);

  const commit = () => {
    const next = draft.trim();
    if (!next) return;
    onEdit(next);
    setEditing(false);
  };

  return (
    <article className="chat-pending-card is-queued">
      <span className="chat-pending-badge">Queued</span>
      {editing ? (
        <textarea
          className="chat-pending-editor"
          aria-label="Queued message text"
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.shiftKey) {
              event.preventDefault();
              commit();
            }
            if (event.key === "Escape") {
              setDraft(item.text);
              setEditing(false);
            }
          }}
        />
      ) : (
        <p className="chat-pending-text">{item.text}</p>
      )}
      <div className="chat-pending-actions">
        {editing ? (
          <>
            <button
              type="button"
              aria-label="Save queued message"
              onClick={commit}
              disabled={!draft.trim()}
            >
              <CheckIcon />
            </button>
            <button
              type="button"
              aria-label="Cancel edit"
              onClick={() => {
                setDraft(item.text);
                setEditing(false);
              }}
            >
              <XIcon />
            </button>
          </>
        ) : (
          <>
            <button
              type="button"
              aria-label="Send queued message now"
              title={
                canSendNow
                  ? "Send now"
                  : "Waits until the current response finishes"
              }
              disabled={!canSendNow}
              onClick={onSendNow}
            >
              <SendIcon />
            </button>
            <button
              type="button"
              aria-label="Edit queued message"
              onClick={() => setEditing(true)}
            >
              <PencilIcon />
            </button>
            <button
              type="button"
              aria-label="Move queued message up"
              disabled={!canMoveUp}
              onClick={onMoveUp}
            >
              <ArrowUpIcon />
            </button>
            <button
              type="button"
              aria-label="Move queued message down"
              disabled={!canMoveDown}
              onClick={onMoveDown}
            >
              <ArrowDownIcon />
            </button>
            <button
              type="button"
              aria-label="Remove queued message"
              onClick={onRemove}
            >
              <Trash2Icon />
            </button>
          </>
        )}
      </div>
    </article>
  );
}
