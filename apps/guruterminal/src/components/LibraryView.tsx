import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { errorMessage } from "../errors";
import type {
  GuruTerminalBridge,
  GuruSummary,
  LibraryRecord,
  LibrarySummary,
  MemoryKind,
} from "../types";
import { LibraryEmptyState } from "./library/LibraryEmptyState";
import { LibraryHome } from "./library/LibraryHome";
import { LibraryRecordDetail } from "./library/LibraryRecordDetail";
import { LibrarySidebar } from "./library/LibrarySidebar";
import {
  matchesStatusFilter,
  type MemoryStatusFilter,
} from "./library/memoryPresentation";
import { MemoryEditor } from "./MemoryEditor";

type MemoryLocation = { record_id: string };

type Props = {
  bridge: GuruTerminalBridge;
  guru: GuruSummary;
  requestedMemory: MemoryLocation | null;
  onRequestConsumed: () => void;
  onTeachInChat: () => void;
  refreshToken: number;
};

export function LibraryView({
  bridge,
  guru,
  requestedMemory,
  onRequestConsumed,
  onTeachInChat,
  refreshToken,
}: Props) {
  const [query, setQuery] = useState("");
  const [kind, setKind] = useState<MemoryKind | "All">("All");
  const [status, setStatus] = useState<MemoryStatusFilter>("all");
  const [results, setResults] = useState<LibrarySummary[]>([]);
  const [catalog, setCatalog] = useState<LibrarySummary[] | null>(null);
  const [record, setRecord] = useState<LibraryRecord | null>(null);
  const [loading, setLoading] = useState(true);
  const [searching, setSearching] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [viewMode, setViewMode] = useState<"rendered" | "raw">("rendered");
  const [localRefreshToken, setLocalRefreshToken] = useState(0);
  const [editing, setEditing] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [reverting, setReverting] = useState(false);
  const recordIdRef = useRef<string | null>(null);
  const libraryReaderRef = useRef<HTMLDivElement>(null);
  const moreDetailsRef = useRef<HTMLDetailsElement>(null);
  const openRequestRef = useRef(0);
  const guruIdRef = useRef(guru.id);
  const searchQueryRef = useRef(query);
  guruIdRef.current = guru.id;

  const openRecord = useCallback(
    async (recordId: string) => {
      const requestId = ++openRequestRef.current;
      const operationGuruId = guru.id;
      setLoading(true);
      setError(null);
      try {
        const next = await bridge.libraryRead(guru.id, recordId);
        if (
          requestId !== openRequestRef.current ||
          guruIdRef.current !== operationGuruId
        ) {
          return;
        }
        recordIdRef.current = next.id;
        setRecord(next);
        setEditing(false);
        setViewMode("rendered");
        moreDetailsRef.current?.removeAttribute("open");
        window.requestAnimationFrame(() => {
          libraryReaderRef.current?.scrollTo?.({ top: 0, behavior: "auto" });
          libraryReaderRef.current
            ?.querySelector<HTMLElement>(".markdown-view h1")
            ?.focus?.({ preventScroll: true });
        });
      } catch (cause) {
        if (
          requestId === openRequestRef.current &&
          guruIdRef.current === operationGuruId
        ) {
          setError(errorMessage(cause, "Could not open this memory."));
        }
      } finally {
        if (
          requestId === openRequestRef.current &&
          guruIdRef.current === operationGuruId
        ) {
          setLoading(false);
        }
      }
    },
    [bridge, guru.id],
  );

  useEffect(() => {
    const returningHome =
      !query.trim() && searchQueryRef.current.trim().length > 0;
    searchQueryRef.current = query;
    let cancelled = false;
    const openEpoch = openRequestRef.current;
    setSearching(true);
    const timer = window.setTimeout(() => {
      void bridge
        .librarySearch({
          guru_id: guru.id,
          query,
          kinds: kind === "All" ? undefined : [kind],
        })
        .then((next) => {
          if (cancelled) return;
          setResults(next);
          setError(null);
          setSearching(false);
          if (openRequestRef.current !== openEpoch) return;
          if (editing) {
            setLoading(false);
            return;
          }
          if (requestedMemory) {
            setLoading(false);
            return;
          }
          if (!query.trim() && kind === "All") {
            if (returningHome) {
              recordIdRef.current = null;
              setRecord(null);
              setViewMode("rendered");
            }
            setLoading(false);
            return;
          }
          const currentStillVisible = next.some(
            (item) => item.id === recordIdRef.current,
          );
          if (recordIdRef.current && currentStillVisible) {
            setLoading(false);
            return;
          }
          if (next[0]) {
            void openRecord(next[0].id);
          } else {
            recordIdRef.current = null;
            setRecord(null);
            setViewMode("rendered");
            setLoading(false);
          }
        })
        .catch((cause: unknown) => {
          if (cancelled) return;
          setError(errorMessage(cause, "Could not search memory."));
          setLoading(false);
          setSearching(false);
        });
    }, 120);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [
    bridge,
    guru.id,
    kind,
    openRecord,
    query,
    refreshToken,
    localRefreshToken,
    requestedMemory,
    editing,
  ]);

  useEffect(() => {
    let cancelled = false;
    void bridge
      .librarySearch({ guru_id: guru.id, query: "" })
      .then((next) => {
        if (!cancelled) setCatalog(next);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [bridge, guru.id, refreshToken, localRefreshToken]);

  useEffect(() => {
    recordIdRef.current = null;
    setRecord(null);
    setQuery("");
    setKind("All");
    setStatus("all");
    setViewMode("rendered");
    setEditing(false);
    setCatalog(null);
    searchQueryRef.current = "";
  }, [guru.id]);

  useEffect(() => {
    if (!requestedMemory) return;
    setViewMode("rendered");
    void openRecord(requestedMemory.record_id).finally(onRequestConsumed);
  }, [onRequestConsumed, openRecord, requestedMemory]);

  const catalogRecords = catalog ?? [];

  const visibleResults = useMemo(() => {
    const source =
      !query.trim() && kind === "All" && !results.length
        ? catalogRecords
        : results;
    return source.filter((item) => matchesStatusFilter(item, status));
  }, [catalogRecords, kind, query, results, status]);

  const copyId = async () => {
    if (!record) return;
    try {
      await navigator.clipboard.writeText(record.id);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1400);
    } catch {
      setCopied(false);
    }
  };

  const recordCount = catalog?.length ?? guru.record_count;
  const libraryIsEmpty =
    !error &&
    recordCount === 0 &&
    !query.trim() &&
    kind === "All" &&
    status === "all";

  const goHome = () => {
    recordIdRef.current = null;
    setRecord(null);
    setEditing(false);
    setQuery("");
    setKind("All");
    setStatus("all");
  };

  const retry = () => setLocalRefreshToken((value) => value + 1);

  const revertRecord = async () => {
    if (!record) return;
    setReverting(true);
    setError(null);
    try {
      await bridge.libraryMemoryRevert({
        guru_id: guru.id,
        record_id: record.id,
        expected_markdown: record.markdown,
      });
      setLocalRefreshToken((value) => value + 1);
      try {
        const next = await bridge.libraryRead(guru.id, record.id);
        recordIdRef.current = next.id;
        setRecord(next);
        setViewMode("rendered");
      } catch {
        recordIdRef.current = null;
        setRecord(null);
        setEditing(false);
      }
    } catch (cause) {
      setError(errorMessage(cause, "Could not revert this memory."));
    } finally {
      setReverting(false);
    }
  };

  const deleteRecord = async () => {
    if (!record) return;
    if (!window.confirm(`Delete “${record.title}”?`)) {
      return;
    }
    setDeleting(true);
    setError(null);
    try {
      await bridge.libraryMemoryDelete({
        guru_id: guru.id,
        record_id: record.id,
      });
      recordIdRef.current = null;
      setRecord(null);
      setLocalRefreshToken((value) => value + 1);
    } catch (cause) {
      setError(errorMessage(cause, "Could not delete this memory."));
    } finally {
      setDeleting(false);
    }
  };

  return (
    <section
      className="library-page"
      aria-label="Guru Memory workspace"
      aria-busy={loading || searching}
    >
      <LibrarySidebar
        query={query}
        kind={kind}
        status={status}
        results={results}
        visibleResults={visibleResults}
        catalog={catalogRecords}
        record={record}
        loading={loading}
        searching={searching}
        error={error}
        recordCount={recordCount}
        libraryIsEmpty={libraryIsEmpty}
        onQueryChange={setQuery}
        onKindChange={setKind}
        onStatusChange={setStatus}
        onHome={goHome}
        onOpenRecord={(recordId) => void openRecord(recordId)}
        onRetry={retry}
      />

      <div className="library-reader" ref={libraryReaderRef} tabIndex={-1}>
        {editing && record ? (
          <MemoryEditor
            bridge={bridge}
            guruId={guru.id}
            record={record}
            onCancel={() => setEditing(false)}
            onSaved={(recordId) => {
              recordIdRef.current = recordId;
              setEditing(false);
              setLocalRefreshToken((value) => value + 1);
              window.setTimeout(() => void openRecord(recordId), 0);
            }}
          />
        ) : loading && !record ? (
          <div
            className="reader-loading"
            role="status"
            aria-label="Loading memory"
          >
            <span />
            <span />
            <span />
          </div>
        ) : record ? (
          <LibraryRecordDetail
            record={record}
            catalog={catalogRecords}
            viewMode={viewMode}
            copied={copied}
            reverting={reverting}
            deleting={deleting}
            moreDetailsRef={moreDetailsRef}
            onEdit={() => setEditing(true)}
            onRevert={() => void revertRecord()}
            onDelete={() => void deleteRecord()}
            onCopyId={() => void copyId()}
            onViewMode={setViewMode}
            onOpenRecord={(recordId) => void openRecord(recordId)}
          />
        ) : libraryIsEmpty ? (
          <LibraryEmptyState onTeach={onTeachInChat} />
        ) : (
          <LibraryHome catalog={catalogRecords} onKindChange={setKind} />
        )}
      </div>
    </section>
  );
}
