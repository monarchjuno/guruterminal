import { compactDate, kindLabel } from "../../format";
import type { LibraryRecord } from "../../types";
import { MarkdownView } from "../MarkdownView";

type Props = {
  record: LibraryRecord;
};

export function WorkspaceMemoryPreview({ record }: Props) {
  return (
    <article className="workspace-memory-view" aria-label="Memory preview">
      <header>
        <div>
          <span className={`kind-badge ${record.kind.toLowerCase()}`}>
            {kindLabel[record.kind]}
          </span>
          {record.as_of ? (
            <time dateTime={record.as_of}>
              As of {compactDate(record.as_of)}
            </time>
          ) : null}
        </div>
      </header>
      <div className="record-content workspace-memory-content">
        <MarkdownView
          markdown={record.markdown}
          idPrefix={`workspace-${record.id}`}
          showFrontmatter={false}
          showOutline={false}
        />
      </div>
    </article>
  );
}
