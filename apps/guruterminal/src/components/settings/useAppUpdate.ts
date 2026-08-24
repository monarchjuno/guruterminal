import { useCallback, useEffect, useState } from "react";
import { errorMessage } from "../../errors";
import type {
  GuruTerminalBridge,
  UpdateInstallRequest,
  UpdateState,
} from "../../types";

const statusWithPhase = (
  status: UpdateState | null,
  phase: UpdateState["phase"],
): UpdateState | null => (status ? { ...status, phase, error: null } : status);

export function useAppUpdate(bridge: GuruTerminalBridge) {
  const [status, setStatus] = useState<UpdateState | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const refreshStatus = useCallback(async () => {
    try {
      setStatus(await bridge.updateStatus());
      setStatusError(null);
    } catch (cause) {
      setStatusError(errorMessage(cause, "Could not read update status."));
    }
  }, [bridge]);

  const checkForUpdates = useCallback(async () => {
    setStatus((current) => statusWithPhase(current, "checking"));
    setActionError(null);
    try {
      const nextStatus = await bridge.updateCheck();
      setStatus(nextStatus);
      setStatusError(null);
    } catch (cause) {
      setStatus((current) => statusWithPhase(current, "idle"));
      setActionError(errorMessage(cause, "Could not check for updates."));
      await refreshStatus();
    }
  }, [bridge, refreshStatus]);

  const installUpdate = useCallback(
    async (request: UpdateInstallRequest) => {
      setStatus((current) => statusWithPhase(current, "confirming"));
      setActionError(null);
      try {
        const result = await bridge.updateInstall(request);
        setStatus((current) =>
          current
            ? {
                ...current,
                phase: "idle",
                blockers: result.blockers,
                error: null,
              }
            : current,
        );
        setStatusError(null);
      } catch (cause) {
        setStatus((current) => statusWithPhase(current, "idle"));
        setActionError(errorMessage(cause, "Could not install the update."));
        await refreshStatus();
      }
    },
    [bridge, refreshStatus],
  );

  useEffect(() => {
    void refreshStatus();
  }, [refreshStatus]);

  useEffect(() => {
    const busy = status?.phase !== undefined && status.phase !== "idle";
    const interval = window.setInterval(
      () => void refreshStatus(),
      busy ? 250 : 15_000,
    );
    return () => window.clearInterval(interval);
  }, [refreshStatus, status?.phase]);

  return {
    status,
    phase: status?.phase ?? "idle",
    error: actionError ?? statusError ?? status?.error ?? null,
    checkForUpdates,
    installUpdate,
  };
}
