import {
  useEffect,
  useRef,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { SearchIcon, XIcon } from "lucide-react";
import { kindLabel } from "../../format";
import type { LibraryRecord, LibrarySummary, MemoryKind } from "../../types";
import {
  MEMORY_KIND_ORDER,
  MEMORY_ROLE_LABEL,
  countByKind,
  formatAsOf,
  groupByRole,
  isRevoked,
  kindFilterLabel,
  memoryRole,
  type MemoryStatusFilter,
} from "./memoryPresentation";

type Props = {
  query: string;
  kind: MemoryKind | "All";
  status: MemoryStatusFilter;
  results: LibrarySummary[];
  visibleResults: LibrarySummary[];
  catalog: LibrarySummary[];
  record: LibraryRecord | null;
  loading: boolean;
  searching: boolean;
  error: string | null;
  recordCount: number;
  libraryIsEmpty: boolean;
  onQueryChange: (query: string) => void;
  onKindChange: (kind: MemoryKind | "All") => void;
  onStatusChange: (status: MemoryStatusFilter) => void;
  onHome: () => void;
  onOpenRecord: (recordId: string) => void;
  onRetry: () => void;
};

const isEditableTarget = (target: EventTarget | null) => {
  if (!(target instanceof HTMLElement)) return false;
  return (
    target.isContentEditable ||
    target.tagName === "INPUT" ||
    target.tagName === "TEXTAREA" ||
    target.tagName === "SELECT"
  );
};

export function LibrarySidebar({
  query,
  kind,
  status,
  results,
  visibleResults,
  catalog,
  record,
  loading,
  searching,
  error,
  recordCount,
  libraryIsEmpty,
  onQueryChange,
  onKindChange,
  onStatusChange,
  onHome,
  onOpenRecord,
  onRetry,
}: Props) {
  const searchInputRef = useRef<HTMLInputElement>(null);
  const grouped = groupByRole(visibleResults);
  const counts = countByKind(catalog);
  const unusedCount = catalog.filter(isRevoked).length;
  const showGroups = kind === "All";
  const summary =
    !query.trim() &&
    kind === "All" &&
    status === "all" &&
    recordCount > results.length
      ? `Showing ${results.length} of ${recordCount}`
      : `${visibleResults.length} ${visibleResults.length === 1 ? "result" : "results"}`;

  useEffect(() => {
    const handleShortcut = (event: KeyboardEvent) => {
      if (
        event.defaultPrevented ||
        event.isComposing ||
        event.metaKey ||
        event.ctrlKey ||
        event.altKey ||
        document.querySelector('[role="dialog"]')
      ) {
        return;
      }

      const panel = searchInputRef.current?.closest(".app-panel");
      if (panel instanceof HTMLElement && (panel.hidden || panel.inert)) return;

      if (
        event.key === "/" &&
        !event.shiftKey &&
        !isEditableTarget(event.target)
      ) {
        event.preventDefault();
        searchInputRef.current?.focus();
        return;
      }

      if (
        event.key === "Escape" &&
        event.target === searchInputRef.current &&
        !query
      ) {
        event.preventDefault();
        searchInputRef.current?.blur();
      }
    };

    window.addEventListener("keydown", handleShortcut);
    return () => window.removeEventListener("keydown", handleShortcut);
  }, [query]);

  const moveResultFocus = (
    event: ReactKeyboardEvent<HTMLElement>,
    delta: number,
  ) => {
    const items = Array.from(
      event.currentTarget.querySelectorAll<HTMLButtonElement>(
        "[data-library-result]",
      ),
    );
    if (!items.length) return;
    const current = items.indexOf(document.activeElement as HTMLButtonElement);
    const nextIndex =
      current < 0
        ? delta > 0
          ? 0
          : items.length - 1
        : Math.max(0, Math.min(items.length - 1, current + delta));
    event.preventDefault();
    items[nextIndex]?.focus();
  };

  return (
    <aside className="library-sidebar">
      <div className="library-title">
        <h1>
          <button
            type="button"
            className="library-home-button"
            aria-label="Show all memories"
            onClick={onHome}
          >
            Memory
          </button>
        </h1>
        <p>{recordCount === 1 ? "1 page." : `${recordCount} pages.`}</p>
      </div>

      <label className="search-box">
        <SearchIcon />
        <span className="sr-only">Search memory</span>
        <input
          ref={searchInputRef}
          value={query}
          placeholder="Search memory"
          aria-controls="library-results"
          onChange={(event) => onQueryChange(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Escape" && query) {
              event.preventDefault();
              onQueryChange("");
            }
            if (event.key === "ArrowDown") {
              const first = event.currentTarget
                .closest(".library-sidebar")
                ?.querySelector<HTMLButtonElement>("[data-library-result]");
              if (first) {
                event.preventDefault();
                first.focus();
              }
            }
          }}
        />
        {query ? (
          <button
            type="button"
            aria-label="Clear search"
            onClick={() => onQueryChange("")}
          >
            <XIcon />
          </button>
        ) : null}
      </label>

      <div className="library-filters">
        <div
          className="library-kind-filters"
          role="toolbar"
          aria-label="Filter memory by type"
        >
          <button
            type="button"
            aria-pressed={kind === "All"}
            className={kind === "All" ? "active" : ""}
            onClick={() => onKindChange("All")}
          >
            {kindFilterLabel("All")}
          </button>
          {MEMORY_KIND_ORDER.map((item) => (
            <button
              type="button"
              key={item}
              aria-pressed={kind === item}
              aria-label={item}
              className={`kind-${item.toLowerCase()}${kind === item ? " active" : ""}`}
              onClick={() => onKindChange(item)}
            >
              {kindLabel[item]}
              <span aria-hidden="true">{counts[item]}</span>
            </button>
          ))}
        </div>
        <div
          className="library-status-filters"
          aria-label="Filter memory by status"
        >
          {(
            [
              ["all", "All statuses"],
              ["active", "Active"],
              ["unused", "Unused"],
            ] as const
          ).map(([value, label]) => (
            <button
              type="button"
              key={value}
              aria-pressed={status === value}
              className={status === value ? "active" : ""}
              onClick={() => onStatusChange(value)}
            >
              {label}
              {value === "unused" ? (
                <span aria-hidden="true">{unusedCount}</span>
              ) : null}
            </button>
          ))}
        </div>
      </div>

      <div className="result-summary" aria-live="polite">
        <span>{summary}</span>
        {query ? <small>“{query}”</small> : null}
      </div>

      {error ? (
        <div className="library-sidebar-error" role="alert">
          <span>{error}</span>
          <button type="button" onClick={onRetry}>
            Try again
          </button>
        </div>
      ) : null}

      <nav
        className="library-results"
        id="library-results"
        aria-label="Memory search results"
        aria-busy={searching || loading}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown") moveResultFocus(event, 1);
          if (event.key === "ArrowUp") moveResultFocus(event, -1);
          if (event.key === "Home") moveResultFocus(event, -999);
          if (event.key === "End") moveResultFocus(event, 999);
        }}
      >
        {searching && !visibleResults.length ? (
          <div className="library-results-status" role="status">
            Searching memory
          </div>
        ) : null}
        {showGroups ? (
          <>
            <ResultGroup
              title={MEMORY_ROLE_LABEL.learned}
              items={grouped.learned}
              selectedId={record?.id}
              onOpenRecord={onOpenRecord}
            />
            <ResultGroup
              title={MEMORY_ROLE_LABEL.input}
              items={grouped.inputs}
              selectedId={record?.id}
              onOpenRecord={onOpenRecord}
            />
          </>
        ) : (
          <ResultGroup
            title={`${kind} · ${MEMORY_ROLE_LABEL[memoryRole(kind)]}`}
            items={visibleResults}
            selectedId={record?.id}
            onOpenRecord={onOpenRecord}
          />
        )}
        {!visibleResults.length && !searching && !libraryIsEmpty ? (
          <div className="no-results">
            <SearchIcon />
            <strong>
              {query
                ? "No matching memories"
                : status === "unused"
                  ? "No unused pages"
                  : kind === "All"
                    ? "No matching memories"
                    : `No ${kind} pages yet`}
            </strong>
            <span>
              {query
                ? "Try another search or memory type."
                : status === "unused"
                  ? "Revoked Wiki and Lens stay listed so you can see what this Guru stopped using."
                  : "Try another type, or teach this Guru a domain in Chat."}
            </span>
          </div>
        ) : null}
      </nav>
    </aside>
  );
}

function ResultGroup({
  title,
  items,
  selectedId,
  onOpenRecord,
}: {
  title: string;
  items: LibrarySummary[];
  selectedId?: string;
  onOpenRecord: (recordId: string) => void;
}) {
  if (!items.length) return null;
  return (
    <section className="library-result-group">
      <h2>{title}</h2>
      <ul>
        {items.map((item) => {
          const unused = isRevoked(item);
          const role = MEMORY_ROLE_LABEL[memoryRole(item.kind)];
          return (
            <li key={item.id}>
              <button
                type="button"
                data-library-result=""
                data-kind={item.kind.toLowerCase()}
                className={[
                  selectedId === item.id ? "active" : "",
                  item.excerpt ? "has-excerpt" : "",
                ]
                  .filter(Boolean)
                  .join(" ")}
                aria-current={selectedId === item.id ? "page" : undefined}
                aria-label={`Open ${item.title} (${kindLabel[item.kind]}, ${role}${unused ? ", unused" : ""})`}
                onClick={() => onOpenRecord(item.id)}
              >
                <div className="result-row-heading">
                  <i className="library-kind-dot" aria-hidden="true" />
                  <strong>{item.title}</strong>
                  {item.as_of ? (
                    <time dateTime={item.as_of}>{formatAsOf(item.as_of)}</time>
                  ) : (
                    <span className="missing-as-of">No date</span>
                  )}
                </div>
                <div className="result-row-meta">
                  <span className={`kind-badge ${item.kind.toLowerCase()}`}>
                    {kindLabel[item.kind]}
                  </span>
                  {unused ? (
                    <span className="status-badge unused">Unused</span>
                  ) : (
                    <span className="sr-only">{role}</span>
                  )}
                  <p>{item.excerpt}</p>
                </div>
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
