const MEMORY_KINDS = ["Wiki", "Lens", "Evidence", "Decision"] as const;
export type MemoryKind = (typeof MEMORY_KINDS)[number];

export type MemoryRef = {
  record_id: string;
  kind: MemoryKind;
  title: string;
  excerpt: string;
  as_of?: string;
  section?: string;
  access: "search_discovered" | "exact_read";
};

export type MemoryUpdateChange = {
  recordId: string;
  kind: MemoryKind;
  operation: "create" | "revise";
  title: string;
  lesson: string;
  basis: string;
  futureUse: string;
};

export type MemoryUpdateResult = {
  status: "applied" | "no_change";
  commitId: string | null;
  changes: MemoryUpdateChange[];
};

export type LibraryRelation = {
  relation: "uses" | "supports" | "updates" | "contradicts" | "see_also";
  target_id: string;
  target_title: string;
  target_title_source: "record" | "record_id_fallback";
};

export type LibrarySummary = {
  id: string;
  kind: MemoryKind;
  title: string;
  excerpt: string;
  as_of?: string;
  status?: "active" | "revoked";
  revoked_by?: string;
  relationships?: LibraryRelation[];
};

export type LibraryRecord = LibrarySummary & {
  markdown: string;
  relationships: LibraryRelation[];
};

export type LibraryDraft = {
  kind: "Wiki" | "Lens";
  title: string;
  summary: string;
  as_of: string;
  entities: string[];
  aliases: string[];
  tags: string[];
  see_also: string[];
  scope: string;
  assumptions: string;
  counterexamples: string;
  limits: string;
  invalidation_conditions: string;
  body_markdown: string;
};

export type LibraryMemoryCreateRequest = {
  guru_id: string;
  draft: LibraryDraft;
};

export type LibraryMemoryUpdateRequest = {
  guru_id: string;
  record_id: string;
  draft: LibraryDraft;
};

export type LibraryMemoryDeleteRequest = {
  guru_id: string;
  record_id: string;
};

export type LibraryMemoryRevertRequest = {
  guru_id: string;
  record_id: string;
  expected_markdown?: string;
  commit_id?: string;
};

export type LibraryMemoryMutation = {
  commit_id: string;
  record_id: string;
};

export type LibrarySearchRequest = {
  guru_id: string;
  query: string;
  kinds?: MemoryKind[];
};
