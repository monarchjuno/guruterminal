import { compactDate, kindLabel } from "../../format";
import type { LibrarySummary, MemoryKind } from "../../types";

export const MEMORY_KIND_ORDER: MemoryKind[] = [
  "Wiki",
  "Lens",
  "Evidence",
  "Decision",
];

export type MemoryRole = "learned" | "input";
export type MemoryStatusFilter = "all" | "active" | "unused";

export const memoryRole = (kind: MemoryKind): MemoryRole =>
  kind === "Wiki" || kind === "Lens" ? "learned" : "input";

export const MEMORY_ROLE_LABEL: Record<MemoryRole, string> = {
  learned: "Learned state",
  input: "Learning input",
};

export const MEMORY_KIND_DESCRIPTION: Record<MemoryKind, string> = {
  Wiki: "What this Guru has learned about the world and can use again",
  Lens: "How this Guru invests, including lessons, limits, and what would prove it wrong",
  Evidence: "Dated claim dossiers from a research theme, each with a source",
  Decision: "A judgment the Guru can learn from without rewriting history",
};

export const isLearnedKind = (kind: MemoryKind) => memoryRole(kind) === "learned";

export const isRevoked = (record: Pick<LibrarySummary, "status">) =>
  record.status === "revoked";

export const formatAsOf = (value?: string) => {
  if (!value) return null;
  try {
    return compactDate(value);
  } catch {
    return value;
  }
};

export const kindFilterLabel = (kind: MemoryKind | "All") =>
  kind === "All" ? "All types" : kindLabel[kind];

export const matchesStatusFilter = (
  record: LibrarySummary,
  status: MemoryStatusFilter,
) => {
  if (status === "all") return true;
  const unused = isRevoked(record);
  return status === "unused" ? unused : !unused;
};

export const groupByRole = (records: LibrarySummary[]) => ({
  learned: records.filter((record) => memoryRole(record.kind) === "learned"),
  inputs: records.filter((record) => memoryRole(record.kind) === "input"),
});

export const countByKind = (records: LibrarySummary[]) => {
  const counts = {
    Wiki: 0,
    Lens: 0,
    Evidence: 0,
    Decision: 0,
  };
  for (const record of records) counts[record.kind] += 1;
  return counts;
};

export function backlinksFor(
  recordId: string,
  catalog: LibrarySummary[],
): Array<{ source_id: string; source_title: string; relation: string }> {
  const incoming: Array<{
    source_id: string;
    source_title: string;
    relation: string;
  }> = [];
  const seen = new Set<string>();
  const push = (
    source_id: string,
    source_title: string,
    relation: string,
  ) => {
    const key = `${source_id}|${relation}`;
    if (seen.has(key)) return;
    seen.add(key);
    incoming.push({ source_id, source_title, relation });
  };
  for (const record of catalog) {
    if (record.id === recordId) {
      if (record.revoked_by) {
        const source = catalog.find((item) => item.id === record.revoked_by);
        push(
          record.revoked_by,
          source?.title ?? record.revoked_by,
          "revokes",
        );
      }
      continue;
    }
    for (const relation of record.relationships ?? []) {
      if (relation.target_id !== recordId) continue;
      push(record.id, record.title, relation.relation);
    }
  }
  return incoming;
}
