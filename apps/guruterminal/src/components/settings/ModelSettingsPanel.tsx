import {
  BrainCircuitIcon,
  ExternalLinkIcon,
  KeyRoundIcon,
  LoaderCircleIcon,
  LogInIcon,
  LogOutIcon,
  PlusIcon,
  RefreshCwIcon,
  SearchIcon,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { errorMessage } from "../../errors";
import type {
  ConfiguredModel,
  ModelCatalog,
  ModelProviderOption,
  ModelVisibilityUpdateRequest,
  ProviderConfigureRequest,
  ProviderConnectionEvent,
} from "../../types";

type Props = {
  catalog: ModelCatalog | null;
  loadError: string | null;
  onLoadModels: (provider: string) => Promise<ModelCatalog>;
  onConnect: (
    provider: string,
    observer: (event: ProviderConnectionEvent) => void,
  ) => Promise<ModelCatalog>;
  onCancelConnect: () => Promise<void>;
  onOpenConnectBrowser: () => Promise<void>;
  onConfigure: (request: ProviderConfigureRequest) => Promise<ModelCatalog>;
  onUpdateModelVisibility: (
    request: ModelVisibilityUpdateRequest,
  ) => Promise<ModelCatalog>;
  onDisconnect: (provider: string) => Promise<ModelCatalog>;
};

type BusyOperation = "models" | "oauth" | "configure" | "disconnect" | null;

const sourceLabel = (
  source: ModelProviderOption["credential_source"],
): string => (source === "environment" ? "From environment" : "Connected");

const modelsForProvider = (
  catalog: ModelCatalog | null,
  providerId: string,
): ConfiguredModel[] =>
  (catalog?.models ?? []).filter((model) => model.provider === providerId);

const visibilityCopy = (
  models: ConfiguredModel[],
  hidden: ReadonlySet<string>,
  discovered: boolean,
  loading: boolean,
): string => {
  if (loading) return "Loading models…";
  if (!models.length) {
    return discovered ? "No models returned" : "Models not loaded yet";
  }
  const visibleCount = models.filter((model) => !hidden.has(model.id)).length;
  return `${visibleCount} of ${models.length} models shown in Chat`;
};

export function ModelSettingsPanel({
  catalog,
  loadError,
  onLoadModels,
  onConnect,
  onCancelConnect,
  onOpenConnectBrowser,
  onConfigure,
  onUpdateModelVisibility,
  onDisconnect,
}: Props) {
  const [connectOpen, setConnectOpen] = useState(false);
  const [providerId, setProviderId] = useState("");
  const [providerQuery, setProviderQuery] = useState("");
  const [modelQueryByProvider, setModelQueryByProvider] = useState<
    Record<string, string>
  >({});
  const [apiKey, setApiKey] = useState("");
  const [busy, setBusy] = useState<BusyOperation>(null);
  const [busyProviderId, setBusyProviderId] = useState<string | null>(null);
  const [visibilityBusyId, setVisibilityBusyId] = useState<string | null>(null);
  const [discoveredProviderIds, setDiscoveredProviderIds] = useState<
    ReadonlySet<string>
  >(() => new Set());
  const [pendingDisconnect, setPendingDisconnect] =
    useState<ModelProviderOption | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [canOpenBrowser, setCanOpenBrowser] = useState(false);
  const [openingBrowser, setOpeningBrowser] = useState(false);
  const operationRef = useRef(0);
  const operationActiveRef = useRef(false);
  const oauthActiveRef = useRef(false);
  const cancelRequestedRef = useRef(false);
  const onCancelConnectRef = useRef(onCancelConnect);
  onCancelConnectRef.current = onCancelConnect;

  useEffect(
    () => () => {
      operationRef.current += 1;
      if (oauthActiveRef.current) void onCancelConnectRef.current();
    },
    [],
  );

  const sortedProviders = useMemo(() => {
    const list = [...(catalog?.providers ?? [])];
    list.sort(
      (left, right) => Number(right.recommended) - Number(left.recommended),
    );
    return list;
  }, [catalog]);
  const connectedProviders = useMemo(
    () =>
      sortedProviders.filter((option) => option.credential_source !== "missing"),
    [sortedProviders],
  );
  const listedProviders = useMemo(() => {
    const needle = providerQuery.trim().toLocaleLowerCase();
    if (!needle) return sortedProviders;
    return sortedProviders.filter(
      (option) =>
        option.id === providerId ||
        `${option.label} ${option.id}`.toLocaleLowerCase().includes(needle),
    );
  }, [providerId, providerQuery, sortedProviders]);
  const provider = sortedProviders.find((item) => item.id === providerId);
  const oauth = provider?.oauth ?? null;
  const hiddenModels = useMemo(
    () => new Set(catalog?.hidden_model_profile_ids ?? []),
    [catalog],
  );

  const markDiscovered = (nextProviderId: string) => {
    setDiscoveredProviderIds((current) => {
      if (current.has(nextProviderId)) return current;
      const next = new Set(current);
      next.add(nextProviderId);
      return next;
    });
  };

  const resetConnectForm = () => {
    setProviderId("");
    setProviderQuery("");
    setApiKey("");
    setStatus(null);
    setError(null);
    setCanOpenBrowser(false);
    setOpeningBrowser(false);
  };

  const openConnectDialog = (nextProviderId = "") => {
    resetConnectForm();
    setProviderId(nextProviderId);
    setConnectOpen(true);
  };

  const finishOperation = (operation: number) => {
    operationActiveRef.current = false;
    if (operation === operationRef.current) {
      setBusy(null);
      setBusyProviderId(null);
    }
  };

  const refreshModels = async (
    nextProviderId: string,
    options: { silent?: boolean } = {},
  ) => {
    if (busy || operationActiveRef.current) return;
    operationActiveRef.current = true;
    const operation = ++operationRef.current;
    if (!options.silent) setBusy("models");
    setBusyProviderId(nextProviderId);
    setError(null);
    if (!options.silent) setStatus("Refreshing available models…");
    markDiscovered(nextProviderId);
    try {
      const next = await onLoadModels(nextProviderId);
      if (operation !== operationRef.current) return;
      const count = modelsForProvider(next, nextProviderId).length;
      if (!options.silent) {
        setStatus(
          count
            ? `${count} ${count === 1 ? "model" : "models"} available.`
            : "No models are available for this provider.",
        );
      }
    } catch (cause) {
      if (operation !== operationRef.current) return;
      setError(errorMessage(cause, "Could not refresh models."));
      setStatus(null);
    } finally {
      finishOperation(operation);
    }
  };
  const refreshModelsRef = useRef(refreshModels);
  refreshModelsRef.current = refreshModels;

  useEffect(() => {
    if (!catalog || busy !== null) return;
    const pending = connectedProviders.find((option) => {
      const hasModels = catalog.models.some(
        (model) => model.provider === option.id,
      );
      return !hasModels && !discoveredProviderIds.has(option.id);
    });
    if (!pending) return;
    void refreshModelsRef.current(pending.id, { silent: true });
  }, [busy, catalog, connectedProviders, discoveredProviderIds]);

  const connect = async () => {
    if (!provider?.oauth || busy || operationActiveRef.current) return;
    operationActiveRef.current = true;
    oauthActiveRef.current = true;
    cancelRequestedRef.current = false;
    const operation = ++operationRef.current;
    setBusy("oauth");
    setBusyProviderId(provider.id);
    setError(null);
    setStatus("Preparing secure sign-in…");
    setCanOpenBrowser(false);
    setOpeningBrowser(false);
    try {
      await onConnect(provider.id, (event) => {
        if (operation !== operationRef.current) return;
        if (event.type === "opening_browser") setCanOpenBrowser(true);
        setStatus(event.message);
      });
      if (operation !== operationRef.current) return;
      markDiscovered(provider.id);
      setConnectOpen(false);
      resetConnectForm();
    } catch (cause) {
      if (operation !== operationRef.current) return;
      if (cancelRequestedRef.current) {
        setError(null);
        setStatus("Sign-in cancelled. You can try again.");
      } else {
        setError(errorMessage(cause, `Could not connect ${provider.label}.`));
        setStatus(null);
      }
    } finally {
      oauthActiveRef.current = false;
      cancelRequestedRef.current = false;
      setCanOpenBrowser(false);
      setOpeningBrowser(false);
      finishOperation(operation);
    }
  };

  const openConnectBrowser = async () => {
    if (busy !== "oauth" || !canOpenBrowser || openingBrowser) return;
    setOpeningBrowser(true);
    setError(null);
    try {
      await onOpenConnectBrowser();
      setStatus("Sign-in page requested. Continue in your browser.");
    } catch (cause) {
      setError(errorMessage(cause, "Could not open the sign-in page."));
    } finally {
      setOpeningBrowser(false);
    }
  };

  const cancelConnect = async () => {
    if (busy !== "oauth" || !oauthActiveRef.current || cancelRequestedRef.current)
      return;
    cancelRequestedRef.current = true;
    setStatus("Cancelling sign-in…");
    setError(null);
    try {
      await onCancelConnect();
      setStatus("Sign-in cancelled. You can try again.");
    } catch (cause) {
      cancelRequestedRef.current = false;
      setError(errorMessage(cause, "Could not cancel sign-in."));
    }
  };

  const configure = async () => {
    if (!provider?.api_key || busy || operationActiveRef.current) return;
    if (provider.credential_source !== "missing" && !apiKey.trim()) {
      setConnectOpen(false);
      resetConnectForm();
      void refreshModels(provider.id);
      return;
    }
    operationActiveRef.current = true;
    const operation = ++operationRef.current;
    setBusy("configure");
    setBusyProviderId(provider.id);
    setError(null);
    setStatus("Saving the credential and loading models…");
    try {
      await onConfigure({
        provider: provider.id,
        api_key: apiKey.trim() || undefined,
      });
      if (operation !== operationRef.current) return;
      markDiscovered(provider.id);
      setConnectOpen(false);
      resetConnectForm();
    } catch (cause) {
      if (operation !== operationRef.current) return;
      setError(errorMessage(cause, "Could not configure this provider."));
      setStatus(null);
    } finally {
      finishOperation(operation);
    }
  };

  const updateVisibility = async (request: ModelVisibilityUpdateRequest) => {
    if (visibilityBusyId) return;
    setVisibilityBusyId(request.model_profile_id);
    setError(null);
    try {
      await onUpdateModelVisibility(request);
    } catch (cause) {
      setError(errorMessage(cause, "Could not update model visibility."));
    } finally {
      setVisibilityBusyId(null);
    }
  };

  const setProviderVisibility = async (
    nextProviderId: string,
    visible: boolean,
  ) => {
    if (visibilityBusyId || !catalog) return;
    const models = modelsForProvider(catalog, nextProviderId);
    setVisibilityBusyId(nextProviderId);
    setError(null);
    const remainingHidden = new Set(hiddenModels);
    try {
      for (const model of models) {
        const currentlyVisible = !remainingHidden.has(model.id);
        if (currentlyVisible === visible) continue;
        await onUpdateModelVisibility({
          model_profile_id: model.id,
          visible_in_chat: visible,
        });
        if (visible) remainingHidden.delete(model.id);
        else remainingHidden.add(model.id);
      }
    } catch (cause) {
      setError(errorMessage(cause, "Could not update model visibility."));
    } finally {
      setVisibilityBusyId(null);
    }
  };

  const disconnect = async (option: ModelProviderOption) => {
    if (
      option.credential_source !== "saved" ||
      busy ||
      operationActiveRef.current
    )
      return;
    operationActiveRef.current = true;
    const operation = ++operationRef.current;
    setBusy("disconnect");
    setBusyProviderId(option.id);
    setError(null);
    setStatus(`Removing ${option.label}…`);
    try {
      await onDisconnect(option.id);
      if (operation !== operationRef.current) return;
      setPendingDisconnect(null);
      setStatus(`${option.label} was removed.`);
    } catch (cause) {
      if (operation !== operationRef.current) return;
      setError(errorMessage(cause, `Could not remove ${option.label}.`));
      setStatus(null);
    } finally {
      finishOperation(operation);
    }
  };

  const apiKeySubmitLabel = provider
    ? provider.credential_source === "missing"
      ? "Connect and load models"
      : apiKey.trim()
        ? "Replace key"
        : "Reload models"
    : "Connect and load models";

  return (
    <div className="settings-stack">
      {loadError ? (
        <Alert variant="destructive">
          <KeyRoundIcon />
          <AlertTitle>Models unavailable</AlertTitle>
          <AlertDescription>{loadError}</AlertDescription>
        </Alert>
      ) : null}

      <section aria-labelledby="connected-provider-title" className="grid gap-3">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <h2
              id="connected-provider-title"
              className="font-heading text-base font-medium"
            >
              Model providers
            </h2>
            <p className="mt-1 text-sm text-muted-foreground">
              Connect an account or API key, then choose which models appear in
              Chat.
            </p>
          </div>
          {connectedProviders.length ? (
            <Button
              type="button"
              aria-label="Connect provider"
              onClick={() => openConnectDialog()}
            >
              <PlusIcon />
              Connect provider
            </Button>
          ) : null}
        </div>

        {status && !connectOpen ? (
          <Alert>
            <AlertTitle>Provider updated</AlertTitle>
            <AlertDescription>{status}</AlertDescription>
          </Alert>
        ) : null}
        {error && !connectOpen ? (
          <Alert variant="destructive">
            <KeyRoundIcon />
            <AlertTitle>Provider update failed</AlertTitle>
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        ) : null}

        {connectedProviders.length ? (
          <ul className="grid gap-3" aria-label="Connected providers">
            {connectedProviders.map((option) => {
              const providerModels = modelsForProvider(catalog, option.id);
              const needle = (modelQueryByProvider[option.id] ?? "")
                .trim()
                .toLocaleLowerCase();
              const filteredModels = needle
                ? providerModels.filter((model) =>
                    `${model.name} ${model.model}`
                      .toLocaleLowerCase()
                      .includes(needle),
                  )
                : providerModels;
              const loading =
                busyProviderId === option.id &&
                (busy === "models" || busy === null);
              const removing =
                busy === "disconnect" && busyProviderId === option.id;
              const copy = visibilityCopy(
                providerModels,
                hiddenModels,
                discoveredProviderIds.has(option.id),
                loading,
              );
              return (
                <li key={option.id}>
                  <Card>
                    <CardHeader className="border-b">
                      <CardTitle className="flex flex-wrap items-center gap-2">
                        {option.label}
                        <Badge variant="outline">
                          {sourceLabel(option.credential_source)}
                        </Badge>
                      </CardTitle>
                      <CardDescription>{copy}</CardDescription>
                    </CardHeader>
                    <CardContent className="grid gap-3 pt-4">
                      <div className="flex flex-wrap items-center gap-2">
                        <Button
                          type="button"
                          size="sm"
                          variant="outline"
                          aria-label={`Reload models for ${option.label}`}
                          disabled={busy !== null}
                          onClick={() => void refreshModels(option.id)}
                        >
                          {loading ? (
                            <LoaderCircleIcon className="animate-spin" />
                          ) : (
                            <RefreshCwIcon />
                          )}
                          Reload models
                        </Button>
                        {option.api_key || option.oauth ? (
                          <Button
                            type="button"
                            size="sm"
                            variant="ghost"
                            aria-label={
                              option.api_key
                                ? `Replace key for ${option.label}`
                                : `Reconnect ${option.label}`
                            }
                            disabled={busy !== null}
                            onClick={() => openConnectDialog(option.id)}
                          >
                            {option.api_key ? (
                              <KeyRoundIcon />
                            ) : (
                              <LogInIcon />
                            )}
                            {option.api_key ? "Replace key" : "Reconnect"}
                          </Button>
                        ) : null}
                        <Button
                          type="button"
                          size="sm"
                          variant="ghost"
                          aria-label={`Disconnect ${option.label}`}
                          disabled={
                            busy !== null ||
                            option.credential_source !== "saved"
                          }
                          title={
                            option.credential_source === "environment"
                              ? "This provider is connected from the environment."
                              : `Disconnect ${option.label}`
                          }
                          onClick={() => setPendingDisconnect(option)}
                        >
                          {removing ? (
                            <LoaderCircleIcon className="animate-spin" />
                          ) : (
                            <LogOutIcon />
                          )}
                          Disconnect
                        </Button>
                      </div>
                      <div className="flex flex-wrap items-center gap-2">
                        <div className="relative min-w-0 flex-1">
                          <SearchIcon className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
                          <Input
                            aria-label={`Search ${option.label} models`}
                            className="pl-9"
                            value={modelQueryByProvider[option.id] ?? ""}
                            onChange={(event) =>
                              setModelQueryByProvider((current) => ({
                                ...current,
                                [option.id]: event.target.value,
                              }))
                            }
                            placeholder="Search models"
                          />
                        </div>
                        <Button
                          type="button"
                          size="sm"
                          variant="ghost"
                          disabled={
                            visibilityBusyId !== null || !providerModels.length
                          }
                          onClick={() =>
                            void setProviderVisibility(option.id, true)
                          }
                        >
                          Show all
                        </Button>
                        <Button
                          type="button"
                          size="sm"
                          variant="ghost"
                          disabled={
                            visibilityBusyId !== null || !providerModels.length
                          }
                          onClick={() =>
                            void setProviderVisibility(option.id, false)
                          }
                        >
                          Hide all
                        </Button>
                      </div>
                      <div
                        className="max-h-64 overflow-y-auto rounded-xl border"
                        aria-label={`${option.label} models`}
                      >
                        {filteredModels.map((model, index) => {
                          const visible = !hiddenModels.has(model.id);
                          return (
                            <div
                              key={model.id}
                              className={`flex items-center gap-3 px-4 py-3 ${index ? "border-t" : ""}`}
                            >
                              <BrainCircuitIcon className="size-4 shrink-0 text-muted-foreground" />
                              <span className="min-w-0 flex-1">
                                <strong className="block truncate text-sm">
                                  {model.name}
                                </strong>
                                <span className="block truncate text-xs text-muted-foreground">
                                  {model.model}
                                </span>
                              </span>
                              {visibilityBusyId === model.id ? (
                                <LoaderCircleIcon className="size-4 animate-spin text-muted-foreground" />
                              ) : null}
                              <Switch
                                aria-label={`Show ${model.name} in Chat`}
                                checked={visible}
                                disabled={visibilityBusyId !== null}
                                onCheckedChange={(checked) =>
                                  void updateVisibility({
                                    model_profile_id: model.id,
                                    visible_in_chat: checked,
                                  })
                                }
                              />
                            </div>
                          );
                        })}
                        {!filteredModels.length ? (
                          <p className="p-6 text-center text-sm text-muted-foreground">
                            {providerModels.length
                              ? "No matching models."
                              : copy}
                          </p>
                        ) : null}
                      </div>
                    </CardContent>
                  </Card>
                </li>
              );
            })}
          </ul>
        ) : (
          <div className="rounded-xl border border-dashed p-8 text-center">
            <p className="text-sm font-medium">No connected providers</p>
            <p className="mt-1 text-sm text-muted-foreground">
              Connect one to start using models in Chat.
            </p>
            <Button
              type="button"
              className="mt-4"
              aria-label="Connect provider"
              onClick={() => openConnectDialog()}
            >
              <PlusIcon />
              Connect provider
            </Button>
          </div>
        )}
      </section>

      <Dialog
        open={connectOpen}
        onOpenChange={(open) => {
          if (!open && busy === "oauth") {
            void cancelConnect();
            return;
          }
          setConnectOpen(open);
          if (!open) resetConnectForm();
        }}
      >
        <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>Connect a model provider</DialogTitle>
            <DialogDescription>
              Choose a provider and connect with its supported sign-in method.
            </DialogDescription>
          </DialogHeader>

          <div className="grid gap-4 py-1">
            <div className="grid gap-2">
              <Label htmlFor="provider-search">Search providers</Label>
              <div className="relative">
                <SearchIcon className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
                <Input
                  id="provider-search"
                  className="pl-9"
                  value={providerQuery}
                  onChange={(event) => setProviderQuery(event.target.value)}
                  placeholder="Search providers"
                  disabled={busy !== null}
                />
              </div>
            </div>
            <div className="grid gap-2">
              <Label>Provider</Label>
              <Select
                value={providerId}
                onValueChange={(nextProviderId) => {
                  setProviderId(nextProviderId);
                  setApiKey("");
                  setStatus(null);
                  setError(null);
                }}
                disabled={busy !== null || !sortedProviders.length}
              >
                <SelectTrigger aria-label="Provider">
                  <SelectValue placeholder="Choose provider" />
                </SelectTrigger>
                <SelectContent>
                  {listedProviders.map((option) => (
                    <SelectItem key={option.id} value={option.id}>
                      {option.label}
                      {option.recommended ? " · Recommended" : ""}
                      {option.credential_source !== "missing"
                        ? ` · ${sourceLabel(option.credential_source)}`
                        : ""}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            {provider ? (
              <div className="rounded-xl border bg-muted/30 p-4">
                <div className="flex items-start gap-3">
                  {oauth ? (
                    <LogInIcon className="mt-0.5 size-5" />
                  ) : (
                    <KeyRoundIcon className="mt-0.5 size-5" />
                  )}
                  <div className="min-w-0">
                    <div className="flex flex-wrap items-center gap-2">
                      <strong className="text-sm">{provider.label}</strong>
                      {provider.recommended ? <Badge>Recommended</Badge> : null}
                      {provider.credential_source !== "missing" ? (
                        <Badge variant="outline">
                          {sourceLabel(provider.credential_source)}
                        </Badge>
                      ) : null}
                    </div>
                    <p className="mt-1 text-sm text-muted-foreground">
                      {provider.description}
                    </p>
                  </div>
                </div>
              </div>
            ) : null}

            {oauth && busy !== "oauth" ? (
              <Button
                type="button"
                aria-label={oauth.label}
                onClick={() => void connect()}
              >
                <LogInIcon />
                {oauth.label}
              </Button>
            ) : null}

            {oauth && busy === "oauth" ? (
              <div className="provider-oauth-status" role="status">
                <div className="provider-oauth-status-copy">
                  <span className="provider-oauth-status-icon" aria-hidden="true">
                    <LoaderCircleIcon className="animate-spin" />
                  </span>
                  <div>
                    <strong>Finish signing in</strong>
                    <p>{status ?? "Preparing secure sign-in…"}</p>
                  </div>
                </div>
                <div className="provider-oauth-actions">
                  <Button
                    type="button"
                    aria-label="Open sign-in page"
                    size="sm"
                    onClick={() => void openConnectBrowser()}
                    disabled={!canOpenBrowser || openingBrowser}
                  >
                    {openingBrowser ? (
                      <LoaderCircleIcon className="animate-spin" />
                    ) : (
                      <ExternalLinkIcon />
                    )}
                    Open sign-in page
                  </Button>
                  <Button
                    type="button"
                    aria-label="Cancel sign-in"
                    size="sm"
                    variant="ghost"
                    onClick={() => void cancelConnect()}
                    disabled={cancelRequestedRef.current}
                  >
                    Cancel
                  </Button>
                </div>
              </div>
            ) : null}

            {provider?.api_key && busy !== "oauth" ? (
              <div className="grid gap-3">
                <div className="grid gap-2">
                  <Label htmlFor="provider-api-key">
                    {provider.credential_label}
                  </Label>
                  <Input
                    id="provider-api-key"
                    type="password"
                    autoComplete="new-password"
                    value={apiKey}
                    onChange={(event) => setApiKey(event.target.value)}
                    placeholder={
                      provider.credential_source === "missing"
                        ? "Paste key"
                        : "Already connected · paste only to replace"
                    }
                  />
                  <p className="text-xs text-muted-foreground">
                    Saved securely and never shown again.
                  </p>
                </div>
                <Button
                  type="button"
                  onClick={() => void configure()}
                  disabled={
                    busy !== null ||
                    (provider.credential_source === "missing" && !apiKey.trim())
                  }
                >
                  {busy === "configure" ? (
                    <LoaderCircleIcon className="animate-spin" />
                  ) : (
                    <KeyRoundIcon />
                  )}
                  {apiKeySubmitLabel}
                </Button>
              </div>
            ) : null}

            {status && busy !== "oauth" ? (
              <Alert>
                <AlertTitle>Provider status</AlertTitle>
                <AlertDescription>{status}</AlertDescription>
              </Alert>
            ) : null}

            {error ? (
              <Alert variant="destructive">
                <KeyRoundIcon />
                <AlertTitle>Provider setup failed</AlertTitle>
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            ) : null}
          </div>
        </DialogContent>
      </Dialog>

      <Dialog
        open={pendingDisconnect !== null}
        onOpenChange={(open) => {
          if (!open && busy !== "disconnect") setPendingDisconnect(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {pendingDisconnect
                ? `Disconnect ${pendingDisconnect.label}?`
                : "Disconnect provider?"}
            </DialogTitle>
            <DialogDescription>
              {pendingDisconnect
                ? `Saved credentials for ${pendingDisconnect.label} will be removed. Its models will no longer be available in Chat.`
                : "Saved credentials will be removed."}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              disabled={busy === "disconnect"}
              onClick={() => setPendingDisconnect(null)}
            >
              Cancel
            </Button>
            <Button
              type="button"
              variant="destructive"
              disabled={busy === "disconnect" || !pendingDisconnect}
              onClick={() => {
                if (pendingDisconnect) void disconnect(pendingDisconnect);
              }}
            >
              Disconnect
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
