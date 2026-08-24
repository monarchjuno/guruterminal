import { useCallback, useEffect, useRef, useState } from "react";
import type { GuruTerminalBridge, GuruSummary } from "../types";
import { errorMessage } from "../errors";
import { emptyChatThread } from "./emptyChat";
import type { ChatSessions } from "./useChatSessions";

type GuruDirectoryDeps = {
  chat: ChatSessions;
  /** Closes native browser tabs and drops all workspace sessions of a guru. */
  removeGuruSessions: (guruId: string) => Promise<void>;
  /** Hides the workspace panel. */
  closeWorkspace: () => void;
  /** Invoked when a guru selection starts (e.g. to close thread dialogs). */
  onSelectionStarted: () => void;
  /** Invoked when a selection lands or the selected guru disappears (clears pending intents). */
  onSelectionReset: () => void;
};

/** Guru list, selection, and lifecycle actions (create/import/rename/delete/export/recover). */
export function useGuruDirectory(
  bridge: GuruTerminalBridge,
  { chat, removeGuruSessions, closeWorkspace, onSelectionStarted, onSelectionReset }: GuruDirectoryDeps,
) {
  const [gurus, setGurus] = useState<GuruSummary[]>([]);
  const [selectedGuru, setSelectedGuru] = useState<GuruSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [mutationBusy, setMutationBusy] = useState(false);
  const [mutationError, setMutationError] = useState<string | null>(null);
  const [recoveryBusy, setRecoveryBusy] = useState(false);
  const [recoveryError, setRecoveryError] = useState<string | null>(null);
  const guruSelectionRequestRef = useRef(0);
  const desiredGuruIdRef = useRef<string | null>(null);
  const { chatRegistry, runRegistry, setThreadsForGuru, setActiveThreadForGuru } =
    chat;

  const refreshGuruAvailability = useCallback(
    async (guruId: string) => {
      const next = await bridge.guruList();
      const refreshed = next.find((guru) => guru.id === guruId) ?? null;
      setGurus(next);
      if (refreshed && desiredGuruIdRef.current === guruId) {
        setSelectedGuru((current) =>
          current?.id === guruId ? refreshed : current,
        );
      }
      return refreshed;
    },
    [bridge],
  );

  /** Applies a workspace snapshot recovered by the run registry. */
  const applyRecoveredGuru = useCallback((guru: GuruSummary) => {
    if (desiredGuruIdRef.current === guru.id) setSelectedGuru(guru);
  }, []);

  const selectGuru = useCallback(
    async (guruId: string, knownGuru?: GuruSummary) => {
      const requestId = ++guruSelectionRequestRef.current;
      desiredGuruIdRef.current = guruId;
      setLoading(true);
      setError(null);
      setMutationError(null);
      setRecoveryError(null);
      closeWorkspace();
      onSelectionStarted();
      if (knownGuru?.availability.status === "recovery_required") {
        setSelectedGuru(knownGuru);
        setLoading(false);
        return true;
      }
      try {
        const workspace = await bridge.guruSelect(guruId);
        if (requestId !== guruSelectionRequestRef.current) return;
        const nextThreads = workspace.threads.map((thread) =>
          chatRegistry.reconcile(thread),
        );
        if (nextThreads.length === 0) {
          chatRegistry.ensure(emptyChatThread(guruId));
        }
        const previousActive = chat.activeThreadIdsRef.current[guruId];
        const nextActive = nextThreads.some(
          (thread) => thread.id === previousActive,
        )
          ? (previousActive ?? null)
          : (nextThreads[0]?.id ?? null);
        setSelectedGuru(workspace.guru);
        setThreadsForGuru(guruId, nextThreads);
        setActiveThreadForGuru(guruId, nextActive);
        onSelectionReset();
        return true;
      } catch (cause) {
        if (requestId !== guruSelectionRequestRef.current) return;
        setError(errorMessage(cause, "Could not open this Guru."));
        return false;
      } finally {
        if (requestId === guruSelectionRequestRef.current) setLoading(false);
      }
    },
    [
      bridge,
      chat.activeThreadIdsRef,
      chatRegistry,
      closeWorkspace,
      onSelectionStarted,
      onSelectionReset,
      setActiveThreadForGuru,
      setThreadsForGuru,
    ],
  );

  useEffect(() => {
    let active = true;
    void bridge
      .guruList()
      .then(async (next) => {
        if (!active) return;
        setGurus(next);
        await runRegistry.hydrateActivities(next.map((guru) => guru.id));
        if (!active) return;
        const first =
          next.find((guru) => guru.availability.status === "available") ??
          next[0];
        if (first) await selectGuru(first.id, first);
        else setLoading(false);
      })
      .catch((cause: unknown) => {
        if (!active) return;
        setError(errorMessage(cause, "Could not load your Gurus."));
        setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [bridge, runRegistry, selectGuru]);

  const addGuru = async (kind: "create" | "import", name?: string) => {
    setLoading(true);
    setMutationBusy(true);
    setMutationError(null);
    let added: GuruSummary | null = null;
    try {
      added =
        kind === "create"
          ? await bridge.guruCreate({ name: name ?? "Untitled agent" })
          : await bridge.guruImportMemory();

      if (added) {
        const created = added;
        setGurus((current) =>
          current.some((guru) => guru.id === created.id)
            ? current.map((guru) => (guru.id === created.id ? created : guru))
            : [...current, created],
        );
      }

      const next = await bridge.guruList();
      setGurus(next);
      if (added) await selectGuru(added.id);
      else setLoading(false);
      return true;
    } catch (cause) {
      setMutationError(
        added
          ? "The agent was created, but the agent list could not be refreshed."
          : errorMessage(cause, "Could not prepare this agent."),
      );
      setLoading(false);
      return Boolean(added);
    } finally {
      setMutationBusy(false);
    }
  };

  const recoverSelectedGuru = async () => {
    if (
      !selectedGuru ||
      selectedGuru.availability.status !== "recovery_required" ||
      recoveryBusy
    ) {
      return;
    }
    const guruId = selectedGuru.id;
    const action = selectedGuru.availability.action;
    setRecoveryBusy(true);
    setRecoveryError(null);
    try {
      const recovered = await bridge.guruRecover({ guru_id: guruId, action });
      setGurus((current) =>
        current.map((guru) => (guru.id === recovered.id ? recovered : guru)),
      );
      if (desiredGuruIdRef.current === guruId) {
        await selectGuru(guruId, recovered);
      }
    } catch (cause) {
      if (desiredGuruIdRef.current === guruId) {
        setRecoveryError(
          errorMessage(cause, "Memory recovery could not finish safely."),
        );
      }
    } finally {
      setRecoveryBusy(false);
    }
  };

  const renameGuru = async (name: string) => {
    if (!selectedGuru || !name.trim() || mutationBusy) return false;
    const guruAtStart = selectedGuru;
    setMutationBusy(true);
    setMutationError(null);
    try {
      const revised = await bridge.guruRename({
        guru_id: guruAtStart.id,
        name: name.trim(),
      });
      setGurus((current) =>
        current.map((guru) => (guru.id === revised.id ? revised : guru)),
      );
      if (desiredGuruIdRef.current === revised.id) setSelectedGuru(revised);
      return true;
    } catch (cause) {
      setMutationError(errorMessage(cause, "Could not rename this agent."));
      return false;
    } finally {
      setMutationBusy(false);
    }
  };

  const deleteGuru = async () => {
    if (!selectedGuru || mutationBusy) return;
    const guruAtStart = selectedGuru;
    const deletedIndex = gurus.findIndex((guru) => guru.id === guruAtStart.id);
    setMutationBusy(true);
    setMutationError(null);
    try {
      await bridge.guruDelete({ guru_id: guruAtStart.id });
      await removeGuruSessions(guruAtStart.id);
      const next = gurus.filter((guru) => guru.id !== guruAtStart.id);
      setGurus(next);
      chat.removeGuru(guruAtStart.id);

      if (desiredGuruIdRef.current === guruAtStart.id) {
        guruSelectionRequestRef.current += 1;
        desiredGuruIdRef.current = null;
        setSelectedGuru(null);
        onSelectionReset();
        closeWorkspace();
      }

      const replacement =
        next[Math.min(deletedIndex, next.length - 1)] ?? next[0];
      if (replacement) await selectGuru(replacement.id);
      else setLoading(false);
    } catch (cause) {
      setMutationError(errorMessage(cause, "Could not delete this agent."));
    } finally {
      setMutationBusy(false);
    }
  };

  const exportGuru = async () => {
    if (!selectedGuru || mutationBusy) return;
    setMutationBusy(true);
    setMutationError(null);
    try {
      await bridge.guruExportMemory(selectedGuru.id);
    } catch (cause) {
      setMutationError(errorMessage(cause, "Could not export this agent."));
    } finally {
      setMutationBusy(false);
    }
  };

  const updateGuru = useCallback((updated: GuruSummary) => {
    setGurus((current) =>
      current.map((guru) => (guru.id === updated.id ? updated : guru)),
    );
    if (desiredGuruIdRef.current === updated.id) setSelectedGuru(updated);
  }, []);

  const recordLastModel = useCallback((guruId: string, profileId: string) => {
    setSelectedGuru((current) =>
      current?.id === guruId
        ? { ...current, last_model_profile_id: profileId }
        : current,
    );
    setGurus((current) =>
      current.map((guru) =>
        guru.id === guruId
          ? { ...guru, last_model_profile_id: profileId }
          : guru,
      ),
    );
  }, []);

  return {
    gurus,
    selectedGuru,
    loading,
    error,
    setError,
    mutationBusy,
    mutationError,
    setMutationError,
    recoveryBusy,
    recoveryError,
    desiredGuruIdRef,
    refreshGuruAvailability,
    applyRecoveredGuru,
    selectGuru,
    addGuru,
    recoverSelectedGuru,
    renameGuru,
    deleteGuru,
    exportGuru,
    updateGuru,
    recordLastModel,
  };
}

export type GuruDirectory = ReturnType<typeof useGuruDirectory>;
