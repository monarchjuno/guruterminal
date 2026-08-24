import { useEffect, useMemo, useRef, useState } from "react";
import {
  BookOpenIcon,
  CalculatorIcon,
  ChartLineIcon,
  ChevronDownIcon,
  CircleStopIcon,
  ClipboardCheckIcon,
  DatabaseIcon,
  FilePlusIcon,
  FileTextIcon,
  FolderIcon,
  GlobeIcon,
  ListTreeIcon,
  LoaderCircleIcon,
  PencilIcon,
  PlayIcon,
  PuzzleIcon,
  RefreshCwIcon,
  SearchIcon,
  TerminalSquareIcon,
  UploadIcon,
  WrenchIcon,
  type LucideIcon,
} from "lucide-react";
import type {
  ChatMessage,
  ChatProgress,
  ChatProgressCategory,
  ChatProgressItem,
  ChatProgressOperation,
  ChatProgressStatus,
} from "../../types";
import { SafeMessageResponse } from "./SafeMessageResponse";

type Props = {
  progress: ChatProgress;
  status: ChatMessage["status"];
  onOpenLink: (href: string) => Promise<void>;
};

type CommentaryItem = Extract<ChatProgressItem, { kind: "commentary" }>;
type ActionItem = Extract<ChatProgressItem, { kind: "tool" | "system" }>;
type ActionGroup = {
  kind: "group";
  id: string;
  category: ChatProgressCategory;
  items: ActionItem[];
};
type TimelineEntry = CommentaryItem | ActionItem | ActionGroup;

const categoryPresentation: Record<
  ChatProgressCategory,
  { label: string; icon: LucideIcon }
> = {
  web: { label: "Web research", icon: GlobeIcon },
  memory: { label: "Memory", icon: DatabaseIcon },
  capability: { label: "Tools", icon: PuzzleIcon },
  finance: { label: "Finance", icon: ChartLineIcon },
  files: { label: "Files", icon: FolderIcon },
  artifact: { label: "Artifacts", icon: FileTextIcon },
  compute: { label: "Computation", icon: TerminalSquareIcon },
  decision: { label: "Decision", icon: ClipboardCheckIcon },
  system: { label: "System", icon: RefreshCwIcon },
};

const operationIcons: Record<ChatProgressOperation, LucideIcon> = {
  search: SearchIcon,
  read: BookOpenIcon,
  write: FilePlusIcon,
  edit: PencilIcon,
  list: ListTreeIcon,
  calculate: CalculatorIcon,
  publish: UploadIcon,
  execute: PlayIcon,
  submit: ClipboardCheckIcon,
  retry: RefreshCwIcon,
  compact: RefreshCwIcon,
  generic: WrenchIcon,
};

const elapsedLabel = (milliseconds: number) => {
  const totalSeconds = Math.max(0, Math.floor(milliseconds / 1_000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return minutes > 0 ? `${minutes}m ${seconds}s` : `${seconds}s`;
};

const groupedEntries = (items: ChatProgressItem[]): TimelineEntry[] => {
  const entries: TimelineEntry[] = [];
  let index = 0;

  while (index < items.length) {
    const item = items[index]!;
    if (item.kind === "commentary") {
      entries.push(item);
      index += 1;
      continue;
    }

    const run = [item];
    let nextIndex = index + 1;
    while (nextIndex < items.length) {
      const next = items[nextIndex]!;
      if (next.kind === "commentary" || next.category !== item.category) break;
      run.push(next);
      nextIndex += 1;
    }

    if (run.length === 1) {
      entries.push(item);
    } else {
      entries.push({
        kind: "group",
        id: `group-${item.id}`,
        category: item.category,
        items: run,
      });
    }
    index = nextIndex;
  }

  return entries;
};

const aggregateStatus = (items: ActionItem[]): ChatProgressStatus => {
  if (items.some((item) => item.status === "running")) return "running";
  if (items.some((item) => item.status === "failed")) return "failed";
  if (items.some((item) => item.status === "stopped")) return "stopped";
  return "succeeded";
};

const itemDuration = (item: ActionItem, now: number) => {
  if (item.startedAtMs == null) return null;
  const end =
    item.status === "running" ? now : (item.finishedAtMs ?? now);
  return elapsedLabel(end - item.startedAtMs);
};

const statusGlyph = (status: ChatProgressStatus) => {
  if (status === "running") {
    return (
      <LoaderCircleIcon className="chat-progress-spinner" aria-label="Running" />
    );
  }
  if (status === "stopped") {
    return <CircleStopIcon aria-label="Stopped" />;
  }
  return null;
};

function CommentaryRow({
  item,
  isStreaming,
  onOpenLink,
}: {
  item: CommentaryItem;
  isStreaming: boolean;
  onOpenLink: (href: string) => Promise<void>;
}) {
  // Keep every in-flight commentary row plain. A tool or system event can
  // make an earlier commentary non-active while the same response is still
  // streaming, and no partial response should enter the Markdown renderer.
  if (isStreaming) {
    return (
      <div className="chat-progress-commentary">
        <p className="chat-progress-commentary-live">{item.text}</p>
      </div>
    );
  }

  return (
    <div className="chat-progress-commentary">
      <SafeMessageResponse
        text={item.text}
        isAnimating={false}
        onOpenLink={onOpenLink}
      />
    </div>
  );
}

function ActionRow({
  item,
  nested = false,
  now,
  onOpenLink,
}: {
  item: ActionItem;
  nested?: boolean;
  now: number;
  onOpenLink: (href: string) => Promise<void>;
}) {
  const OperationIcon = operationIcons[item.operation];
  const duration = itemDuration(item, now);
  const glyph = statusGlyph(item.status);

  return (
    <div
      className={`chat-progress-entry chat-progress-row ${item.status}${nested ? " chat-progress-child" : ""}`}
      data-progress-category={item.category}
      data-progress-operation={item.operation}
      data-progress-status={item.status}
    >
      <span className="chat-progress-node">
        <OperationIcon aria-hidden="true" />
      </span>
      <div className="chat-progress-copy">
        <span className="chat-progress-action">{item.action}</span>
        {item.target &&
          (item.href ? (
            <button
              type="button"
              className="chat-progress-target chat-progress-link"
              title={item.target}
              onClick={() => void onOpenLink(item.href!)}
            >
              {item.target}
            </button>
          ) : (
            <span className="chat-progress-target" title={item.target}>
              {item.target}
            </span>
          ))}
      </div>
      {duration && <span className="chat-progress-duration">{duration}</span>}
      {glyph && <span className="chat-progress-status">{glyph}</span>}
    </div>
  );
}

function ProgressGroup({
  entry,
  now,
  onOpenLink,
}: {
  entry: ActionGroup;
  now: number;
  onOpenLink: (href: string) => Promise<void>;
}) {
  const groupStatus = aggregateStatus(entry.items);
  const [expanded, setExpanded] = useState(false);
  const presentation = categoryPresentation[entry.category];
  const CategoryIcon = presentation.icon;
  const glyph = statusGlyph(groupStatus);

  return (
    <div
      className="chat-progress-group"
      data-progress-category={entry.category}
    >
      <button
        type="button"
        className="chat-progress-group-toggle"
        aria-expanded={expanded}
        onClick={() => setExpanded((current) => !current)}
      >
        <CategoryIcon aria-hidden="true" />
        <span className="chat-progress-group-label">
          {presentation.label} · {entry.items.length} actions
        </span>
        {glyph && <span className="chat-progress-status">{glyph}</span>}
        <ChevronDownIcon
          className={`chat-progress-chevron ${expanded ? "open" : ""}`}
          aria-hidden="true"
        />
      </button>
      {expanded && (
        <div className="chat-progress-children">
          {entry.items.map((item) => (
            <ActionRow
              item={item}
              key={item.id}
              nested
              now={now}
              onOpenLink={onOpenLink}
            />
          ))}
        </div>
      )}
    </div>
  );
}

export function ChatProgressTimeline({ progress, status, onOpenLink }: Props) {
  const isRunning = status === "streaming";
  const [expanded, setExpanded] = useState(isRunning);
  const [now, setNow] = useState(Date.now());
  const wasRunning = useRef(isRunning);

  useEffect(() => {
    if (isRunning && !wasRunning.current) setExpanded(true);
    if (!isRunning && wasRunning.current) setExpanded(false);
    wasRunning.current = isRunning;
  }, [isRunning]);

  useEffect(() => {
    if (!isRunning) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [isRunning]);

  const actions = useMemo(
    () =>
      progress.items.filter(
        (item): item is ActionItem => item.kind !== "commentary",
      ),
    [progress.items],
  );
  const entries = useMemo(
    () => groupedEntries(progress.items),
    [progress.items],
  );
  const runningActions = actions.filter((item) => item.status === "running");
  const end = isRunning ? now : (progress.finishedAtMs ?? now);
  const elapsed = elapsedLabel(end - progress.startedAtMs);
  const currentWork =
    runningActions.length === 1
      ? runningActions[0].action
      : runningActions.length > 1
        ? `${runningActions.length} actions running`
        : "Preparing response";
  const categoryLabels = [
    ...new Set(actions.map((item) => categoryPresentation[item.category].label)),
  ];
  const hasFailure = actions.some((item) => item.status === "failed");
  const stepLabel =
    actions.length === 1 ? "1 step" : `${actions.length} steps`;
  const heading = isRunning
    ? `Working · ${currentWork} · ${elapsed}`
    : status === "aborted"
      ? `Work stopped · ${stepLabel} · ${elapsed}`
      : status === "error"
        ? `Work failed · ${stepLabel} · ${elapsed}`
        : `${stepLabel}${categoryLabels.length ? ` · ${categoryLabels.join(", ")}` : ""} · ${elapsed}`;
  return (
    <section
      className={`chat-progress${hasFailure && status === "error" ? " has-failure" : ""}`}
      aria-label="Work progress"
      aria-busy={isRunning}
      data-run-status={status ?? "complete"}
    >
      <button
        type="button"
        className="chat-progress-toggle"
        aria-expanded={expanded}
        onClick={() => setExpanded((current) => !current)}
      >
        {isRunning && (
          <LoaderCircleIcon
            className="chat-progress-spinner"
            aria-hidden="true"
          />
        )}
        <span className="chat-progress-heading">{heading}</span>
        <ChevronDownIcon
          className={`chat-progress-chevron ${expanded ? "open" : ""}`}
          aria-hidden="true"
        />
      </button>

      {!expanded && isRunning && (
        <div className="chat-progress-current" aria-live="polite">
          {currentWork} · {elapsed}
        </div>
      )}

      {expanded && (
        <div className="chat-progress-items">
          {entries.map((entry) => {
            if (entry.kind === "commentary") {
              return (
                <CommentaryRow
                  item={entry}
                  key={entry.id}
                  isStreaming={isRunning}
                  onOpenLink={onOpenLink}
                />
              );
            }
            if (entry.kind === "group") {
              return (
                <ProgressGroup
                  entry={entry}
                  key={entry.id}
                  now={now}
                  onOpenLink={onOpenLink}
                />
              );
            }
            return (
              <ActionRow
                item={entry}
                key={entry.id}
                now={now}
                onOpenLink={onOpenLink}
              />
            );
          })}
        </div>
      )}
    </section>
  );
}
