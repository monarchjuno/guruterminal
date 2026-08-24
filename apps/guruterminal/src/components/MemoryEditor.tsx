import { useEffect, useMemo, useState } from "react";
import type {
  GuruTerminalBridge,
  LibraryDraft,
  LibraryRecord,
  LibrarySummary,
} from "../types";
import { errorMessage } from "../errors";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";

type Props = {
  bridge: GuruTerminalBridge;
  guruId: string;
  record: LibraryRecord;
  onCancel: () => void;
  onSaved: (recordId: string) => void;
};

const memoryAsOfNow = () =>
  new Date().toISOString().replace(/\.\d{3}Z$/, "Z");

const list = (markdown: string, key: string) => {
  const lines = markdown.split("\n");
  const start = lines.findIndex((line) => line.trim() === `${key}:`);
  if (start < 0) return [];
  const values: string[] = [];
  for (const line of lines.slice(start + 1)) {
    const match = line.match(/^\s+-\s+(.+)$/);
    if (!match) break;
    values.push(match[1].trim().replace(/^['"]|['"]$/g, "").replace(/''/g, "'"));
  }
  return values;
};

const splitDocument = (markdown: string) => {
  const end = markdown.startsWith("---\n") ? markdown.indexOf("\n---", 4) : -1;
  return end < 0 ? markdown : markdown.slice(end + 4).trim();
};

const extractSection = (body: string, heading: string) => {
  const pattern = new RegExp(`(?:^|\\n)# ${heading}\\n\\n([\\s\\S]*?)(?=\\n# |$)`, "i");
  return (body.match(pattern)?.[1] ?? "").trim();
};

const draftFromRecord = (record: LibraryRecord): LibraryDraft => {
  const body = splitDocument(record.markdown);
  const structured = ["Scope", "Assumptions", "Counterexamples", "Limits", "Invalidation conditions"];
  const remainder = structured.reduce(
    (value, heading) => value.replace(new RegExp(`(?:^|\\n)# ${heading}\\n\\n[\\s\\S]*?(?=\\n# |$)`, "i"), ""),
    body,
  ).trim();
  return {
    kind: record.kind === "Lens" ? "Lens" : "Wiki",
    title: record.title,
    summary: record.excerpt,
    as_of: record.as_of ?? memoryAsOfNow(),
    entities: list(record.markdown, "entities"),
    aliases: list(record.markdown, "aliases"),
    tags: list(record.markdown, "tags"),
    see_also: list(record.markdown, "see_also"),
    scope: extractSection(body, "Scope"),
    assumptions: extractSection(body, "Assumptions"),
    counterexamples: extractSection(body, "Counterexamples"),
    limits: extractSection(body, "Limits"),
    invalidation_conditions: extractSection(body, "Invalidation conditions"),
    body_markdown: remainder,
  };
};

const csv = (value: string) => value.split(",").map((item) => item.trim()).filter(Boolean);

export function MemoryEditor({ bridge, guruId, record, onCancel, onSaved }: Props) {
  const [draft, setDraft] = useState<LibraryDraft>(() => draftFromRecord(record));
  const [similar, setSimilar] = useState<LibrarySummary[]>([]);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const title = draft.title.trim();

  useEffect(() => {
    setDraft(draftFromRecord(record));
  }, [record]);

  useEffect(() => {
    if (title.length < 3) {
      setSimilar([]);
      return;
    }
    let cancelled = false;
    const timer = window.setTimeout(() => {
      void bridge.librarySearch({ guru_id: guruId, query: title, kinds: [draft.kind] })
        .then((items) => {
          if (!cancelled) setSimilar(items.filter((item) => item.id !== record.id).slice(0, 3));
        });
    }, 180);
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [bridge, draft.kind, guruId, record.id, title]);

  const valid = useMemo(() => {
    if (!draft.title.trim() || !draft.summary.trim() || !draft.body_markdown.trim()) return false;
    if (draft.kind === "Lens") {
      return [draft.scope, draft.assumptions, draft.counterexamples, draft.limits, draft.invalidation_conditions]
        .every((value) => value.trim());
    }
    return true;
  }, [draft]);

  const set = <K extends keyof LibraryDraft>(key: K, value: LibraryDraft[K]) =>
    setDraft((current) => ({ ...current, [key]: value }));

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      const result = await bridge.libraryMemoryUpdate({
        guru_id: guruId,
        record_id: record.id,
        draft,
      });
      onSaved(result.record_id);
    } catch (cause) {
      setError(errorMessage(cause, "Could not save this Memory."));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="memory-editor" aria-label="Edit memory">
      <header>
        <div>
          <small>Edit</small>
          <h2>{`Edit ${record.title.trim() || "memory"}`}</h2>
        </div>
        <div className="memory-editor-header-actions">
          <Button type="button" variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button type="button" disabled={!valid || saving} onClick={() => void save()}>
            {saving ? "Saving…" : "Save memory"}
          </Button>
        </div>
      </header>
      {error ? <div className="page-error" role="alert">{error}</div> : null}
      <div className="memory-editor-layout">
        <div className="memory-editor-main">
          <label className="memory-editor-title-field">
            <span className="sr-only">Title</span>
            <Input
              value={draft.title}
              onChange={(event) => set("title", event.target.value)}
              aria-label="Title"
              placeholder="Title"
            />
          </label>
          <label>
            <span>Summary</span>
            <Textarea
              rows={3}
              value={draft.summary}
              onChange={(event) => set("summary", event.target.value)}
            />
          </label>
          <label className="memory-editor-body">
            <span>Markdown body</span>
            <Textarea
              className="memory-markdown-editor"
              rows={18}
              value={draft.body_markdown}
              onChange={(event) => set("body_markdown", event.target.value)}
            />
          </label>
        </div>
        <aside className="memory-editor-meta">
          <div className="memory-editor-kind">
            <span>Type</span>
            <span className={`kind-badge ${draft.kind.toLowerCase()}`}>{draft.kind}</span>
          </div>
          <Label>
            As of
            <Input value={draft.as_of} onChange={(event) => set("as_of", event.target.value)} />
          </Label>
          <Label>
            Aliases
            <Input
              value={draft.aliases.join(", ")}
              onChange={(event) => set("aliases", csv(event.target.value))}
            />
          </Label>
          <Label>
            Entities
            <Input
              value={draft.entities.join(", ")}
              onChange={(event) => set("entities", csv(event.target.value))}
            />
          </Label>
          <Label>
            Tags
            <Input
              value={draft.tags.join(", ")}
              onChange={(event) => set("tags", csv(event.target.value))}
            />
          </Label>
          {draft.kind === "Wiki" ? (
            <Label>
              See also
              <Input
                value={draft.see_also.join(", ")}
                onChange={(event) => set("see_also", csv(event.target.value))}
              />
            </Label>
          ) : null}
          {draft.kind === "Lens" ? (
            <>
              <Label>
                Scope
                <Textarea rows={2} value={draft.scope} onChange={(event) => set("scope", event.target.value)} />
              </Label>
              <Label>
                Assumptions
                <Textarea
                  rows={2}
                  value={draft.assumptions}
                  onChange={(event) => set("assumptions", event.target.value)}
                />
              </Label>
              <Label>
                Counterexamples
                <Textarea
                  rows={2}
                  value={draft.counterexamples}
                  onChange={(event) => set("counterexamples", event.target.value)}
                />
              </Label>
              <Label>
                Limits
                <Textarea rows={2} value={draft.limits} onChange={(event) => set("limits", event.target.value)} />
              </Label>
              <Label>
                Invalidation conditions
                <Textarea
                  rows={2}
                  value={draft.invalidation_conditions}
                  onChange={(event) => set("invalidation_conditions", event.target.value)}
                />
              </Label>
            </>
          ) : null}
        </aside>
      </div>
      {similar.length > 0 ? (
        <aside className="similar-memory">
          <strong>Possible existing matches</strong>
          <span>Consider improving one of these instead of creating a duplicate.</span>
          {similar.map((item) => (
            <code key={item.id}>
              {item.id} · {item.title}
            </code>
          ))}
        </aside>
      ) : null}
      <footer>
        <span>Saved as user-authored Memory. It cannot lower the finance evidence standard.</span>
      </footer>
    </div>
  );
}
