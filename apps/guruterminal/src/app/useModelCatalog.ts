import { useCallback, useEffect, useState } from "react";
import type {
  GuruTerminalBridge,
  ModelCatalog,
  ModelRunSelection,
  ModelVisibilityUpdateRequest,
  ProviderConfigureRequest,
} from "../types";
import { errorMessage } from "../errors";
import { visibleCatalogModels } from "../modelSelection";

/** Model catalog loading and per-guru model run selections. */
export function useModelCatalog(bridge: GuruTerminalBridge) {
  const [catalog, setCatalog] = useState<ModelCatalog | null>(null);
  const [resolved, setResolved] = useState(false);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  const [selections, setSelections] = useState<
    Record<string, ModelRunSelection>
  >({});

  useEffect(() => {
    let active = true;
    void bridge
      .modelCatalogGet()
      .then((settings) => {
        if (!active) return;
        setCatalog(settings);
        setSettingsError(null);
        setResolved(true);
      })
      .catch((cause: unknown) => {
        if (!active) return;
        setSettingsError(errorMessage(cause, "Could not load model settings."));
        setResolved(true);
      });
    return () => {
      active = false;
    };
  }, [bridge]);

  const changeSelection = useCallback(
    (guruId: string, selection: ModelRunSelection) => {
      setSelections((current) => ({
        ...current,
        [guruId]: selection,
      }));
    },
    [],
  );

  const configureProvider = useCallback(
    async (request: ProviderConfigureRequest) => {
      const next = await bridge.providerConfigure(request);
      setCatalog(next);
      setSettingsError(null);
      return next;
    },
    [bridge],
  );

  const updateModelVisibility = useCallback(
    async (request: ModelVisibilityUpdateRequest) => {
      const next = await bridge.modelVisibilityUpdate(request);
      setCatalog(next);
      setSettingsError(null);
      return next;
    },
    [bridge],
  );

  const disconnectProvider = useCallback(
    async (provider: string) => {
      const next = await bridge.providerDisconnect(provider);
      setCatalog(next);
      setSettingsError(null);
      return next;
    },
    [bridge],
  );

  const needsProviderSetup =
    resolved &&
    catalog !== null &&
    catalog.providers.every(
      (provider) => provider.credential_source === "missing",
    ) &&
    catalog.models.every((model) => model.credential_source === "missing");
  const needsVisibleModels =
    resolved &&
    catalog !== null &&
    !needsProviderSetup &&
    visibleCatalogModels(catalog).length === 0;

  return {
    catalog,
    setCatalog,
    resolved,
    settingsError,
    selections,
    changeSelection,
    configureProvider,
    updateModelVisibility,
    disconnectProvider,
    needsProviderSetup,
    needsVisibleModels,
  };
}
