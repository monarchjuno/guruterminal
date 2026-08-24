import type {
  LibraryRecord,
  LibrarySearchRequest,
  LibrarySummary,
  LibraryDraft,
  LibraryMemoryCreateRequest,
  LibraryMemoryDeleteRequest,
  LibraryMemoryMutation,
  LibraryMemoryRevertRequest,
  LibraryMemoryUpdateRequest,
} from "../../types";
import { clone, makeId, type MockBridgeState } from "./state";

export const librarySearch = async (
  state: MockBridgeState,
  request: LibrarySearchRequest,
): Promise<LibrarySummary[]> => {
  const query = request.query.trim().toLocaleLowerCase("en-US");
  const records = state.library[request.guru_id] ?? [];
  return clone(
    records.filter((record) => {
      const kindMatch =
        !request.kinds?.length || request.kinds.includes(record.kind);
      const queryMatch =
        !query ||
        `${record.title} ${record.excerpt} ${record.id}`
          .toLocaleLowerCase("en-US")
          .includes(query);
      return kindMatch && queryMatch;
    }),
  );
};

export const libraryRead = async (
  state: MockBridgeState,
  guru_id: string,
  record_id: string,
): Promise<LibraryRecord> => {
  const record = (state.library[guru_id] ?? []).find(
    (item) => item.id === record_id,
  );
  if (!record) throw new Error(`Record not found: ${record_id}`);
  return clone(record);
};

const draftMarkdown = (draft: LibraryDraft, id: string) =>
  `---\nid: ${id}\ntitle: '${draft.title}'\nsummary: '${draft.summary}'\nas_of: ${draft.as_of}\n---\n\n${draft.body_markdown}`;

const mutation = (recordId: string): LibraryMemoryMutation => ({
  commit_id: makeId("commit-user"),
  record_id: recordId,
});

const stashPrevious = (
  state: MockBridgeState,
  guruId: string,
  recordId: string,
  markdown: string,
) => {
  (state.memoryPrevious[guruId] ??= {})[recordId] = markdown;
};

export const libraryMemoryCreate = async (
  state: MockBridgeState,
  request: LibraryMemoryCreateRequest,
): Promise<LibraryMemoryMutation> => {
  const id = `${request.draft.kind.toLowerCase()}:${makeId("user")}`;
  const markdown = draftMarkdown(request.draft, id);
  (state.library[request.guru_id] ??= []).unshift({
    id,
    kind: request.draft.kind,
    title: request.draft.title,
    excerpt: request.draft.summary,
    as_of: request.draft.as_of,
    markdown,
    relationships: [],
  });
  stashPrevious(state, request.guru_id, id, "");
  return mutation(id);
};

export const libraryMemoryUpdate = async (
  state: MockBridgeState,
  request: LibraryMemoryUpdateRequest,
): Promise<LibraryMemoryMutation> => {
  const record = (state.library[request.guru_id] ?? []).find((item) => item.id === request.record_id);
  if (!record || !["Wiki", "Lens"].includes(record.kind)) throw new Error("Only Wiki and Lens can be edited.");
  stashPrevious(state, request.guru_id, record.id, record.markdown);
  record.title = request.draft.title;
  record.excerpt = request.draft.summary;
  record.as_of = request.draft.as_of;
  record.markdown = draftMarkdown(request.draft, record.id);
  return mutation(record.id);
};

export const libraryMemoryDelete = async (
  state: MockBridgeState,
  request: LibraryMemoryDeleteRequest,
): Promise<LibraryMemoryMutation> => {
  const records = state.library[request.guru_id] ?? [];
  const index = records.findIndex((item) => item.id === request.record_id);
  if (index < 0) throw new Error("Memory record not found.");
  const [record] = records.splice(index, 1);
  if (state.memoryPrevious[request.guru_id]) {
    delete state.memoryPrevious[request.guru_id][record.id];
  }
  return mutation(record.id);
};

export const libraryMemoryRevert = async (
  state: MockBridgeState,
  request: LibraryMemoryRevertRequest,
): Promise<LibraryMemoryMutation> => {
  const records = state.library[request.guru_id] ?? [];
  const record = records.find((item) => item.id === request.record_id);
  if (!record || !["Wiki", "Lens"].includes(record.kind)) {
    throw new Error("Only Wiki and Lens can be reverted.");
  }
  if (
    request.expected_markdown != null &&
    request.expected_markdown !== record.markdown
  ) {
    throw new Error("memory changed after the write was prepared");
  }
  const previous = state.memoryPrevious[request.guru_id]?.[record.id];
  if (previous == null) {
    throw new Error("Memory has no previous version");
  }
  if (previous === "") {
    const index = records.findIndex((item) => item.id === record.id);
    if (index >= 0) records.splice(index, 1);
  } else {
    record.markdown = previous;
  }
  delete state.memoryPrevious[request.guru_id][record.id];
  return mutation(record.id);
};
