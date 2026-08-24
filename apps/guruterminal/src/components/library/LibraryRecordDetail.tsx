import type { RefObject } from "react";
import {
  CheckIcon,
  CopyIcon,
  EllipsisIcon,
  PencilIcon,
  RotateCcwIcon,
  Trash2Icon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { kindLabel } from "../../format";
import type { LibraryRecord, LibrarySummary } from "../../types";
import { MarkdownView } from "../MarkdownView";
import {
  MEMORY_ROLE_LABEL,
  backlinksFor,
  formatAsOf,
  isLearnedKind,
  isRevoked,
  memoryRole,
} from "./memoryPresentation";

type Props = {
  record: LibraryRecord;
  catalog: LibrarySummary[];
  viewMode: "rendered" | "raw";
  copied: boolean;
  reverting: boolean;
  deleting: boolean;
  moreDetailsRef: RefObject<HTMLDetailsElement | null>;
  onEdit: () => void;
  onRevert: () => void;
  onDelete: () => void;
  onCopyId: () => void;
  onViewMode: (mode: "rendered" | "raw") => void;
  onOpenRecord: (recordId: string) => void;
};

const RELATION_ORDER = [
  "uses",
  "supports",
  "updates",
  "contradicts",
  "see_also",
] as const;

const RELATION_LABEL: Record<(typeof RELATION_ORDER)[number], string> = {
  uses: "Uses",
  supports: "Supports",
  updates: "Updates",
  contradicts: "Contradicts",
  see_also: "See also",
};

const groupRelationships = (relationships: LibraryRecord["relationships"]) =>
  RELATION_ORDER.flatMap((relation) => {
    const items = relationships.filter((item) => item.relation === relation);
    return items.length ? [{ relation, items }] : [];
  });

export function LibraryRecordDetail({
  record,
  catalog,
  viewMode,
  copied,
  reverting,
  deleting,
  moreDetailsRef,
  onEdit,
  onRevert,
  onDelete,
  onCopyId,
  onViewMode,
  onOpenRecord,
}: Props) {
  const role = memoryRole(record.kind);
  const learned = isLearnedKind(record.kind);
  const unused = isRevoked(record);
  const backlinks = backlinksFor(record.id, catalog);
  const relationshipGroups = groupRelationships(record.relationships);

  return (
    <>
      <header className="record-heading" tabIndex={-1}>
        <div className="record-heading-row">
          <div className="record-meta">
            <span className={`kind-badge ${record.kind.toLowerCase()}`}>
              {kindLabel[record.kind]}
            </span>
            <span className={`record-role ${role}`}>
              {MEMORY_ROLE_LABEL[role]}
              {learned ? "" : " · sealed"}
            </span>
            <span>
              {record.as_of ? `As of ${formatAsOf(record.as_of)}` : "No date"}
            </span>
          </div>
          <div className="record-actions">
            {learned ? (
              <>
                <Button type="button" variant="outline" size="sm" onClick={onEdit}>
                  <PencilIcon />
                  Edit
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  disabled={reverting}
                  onClick={onRevert}
                >
                  <RotateCcwIcon />
                  {reverting ? "Reverting…" : "Revert"}
                </Button>
              </>
            ) : null}
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="danger-text"
              disabled={deleting}
              onClick={onDelete}
            >
              <Trash2Icon />
              {deleting ? "Deleting…" : "Delete"}
            </Button>
            <details className="record-more" ref={moreDetailsRef}>
              <summary>
                <EllipsisIcon />
                More
              </summary>
              <div className="record-more-menu">
                <span>Record ID</span>
                <div className="record-id">
                  <code>{record.id}</code>
                  <Button
                    type="button"
                    variant="outline"
                    size="xs"
                    onClick={onCopyId}
                    aria-label="Copy record ID"
                  >
                    {copied ? <CheckIcon /> : <CopyIcon />}
                    {copied ? "Copied" : "Copy ID"}
                  </Button>
                </div>
              </div>
            </details>
          </div>
        </div>
        {unused ? (
          <p className="record-superseded" role="status">
            Unused. This claim is superseded.
          </p>
        ) : null}
        {!learned ? (
          <p className="record-sealed" role="note">
            Chat can learn from this page without rewriting it.
          </p>
        ) : null}
        <div className="segmented-control" aria-label="Markdown view">
          <button
            type="button"
            aria-pressed={viewMode === "rendered"}
            className={viewMode === "rendered" ? "active" : ""}
            onClick={() => onViewMode("rendered")}
          >
            Rendered
          </button>
          <button
            type="button"
            aria-pressed={viewMode === "raw"}
            className={viewMode === "raw" ? "active" : ""}
            onClick={() => onViewMode("raw")}
          >
            Raw
          </button>
        </div>
      </header>
      <div className="record-content">
        {viewMode === "rendered" ? (
          <MarkdownView markdown={record.markdown} idPrefix={record.id} />
        ) : (
          <pre className="raw-markdown">
            <code>{record.markdown}</code>
          </pre>
        )}
      </div>
      <footer className="record-relations">
        <div>
          <h2>Related</h2>
        </div>
        {relationshipGroups.length ? (
          <div className="relation-list">
            {relationshipGroups.map((group) => (
              <section className="relation-group" key={group.relation}>
                <h3>{RELATION_LABEL[group.relation]}</h3>
                <div className="relation-group-items">
                  {group.items.map((relation) => (
                    <button
                      type="button"
                      key={`${relation.relation}-${relation.target_id}`}
                      aria-label={`Open related memory: ${relation.target_title}`}
                      onClick={() => onOpenRecord(relation.target_id)}
                    >
                      <strong>{relation.target_title}</strong>
                      {relation.target_title_source === "record_id_fallback" ? (
                        <small>Title unavailable</small>
                      ) : null}
                    </button>
                  ))}
                </div>
              </section>
            ))}
          </div>
        ) : (
          <p className="no-relations">No related notes yet.</p>
        )}
        <div>
          <h2>Backlinks</h2>
        </div>
        {backlinks.length ? (
          <div className="relation-list">
            {backlinks.map((link) => (
              <button
                type="button"
                key={`${link.source_id}-${link.relation}`}
                aria-label={`Open related memory: ${link.source_title}`}
                onClick={() => onOpenRecord(link.source_id)}
              >
                <span>{link.relation}</span>
                <strong>{link.source_title}</strong>
              </button>
            ))}
          </div>
        ) : (
          <p className="no-relations">No backlinks yet.</p>
        )}
      </footer>
    </>
  );
}
