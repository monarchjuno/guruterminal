import { DatabaseIcon, RefreshCwIcon, SearchIcon } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type {
  MarketplaceEntry,
  MarketplaceFreeState,
  MarketplaceSnapshot,
  MarketplaceSource,
} from "../../marketplace/types";
import type { GuruTerminalBridge } from "../../types";
import { MarketplaceCard } from "./MarketplaceCard";
import { MarketplaceSetupDialog } from "./MarketplaceSetupDialog";

type Props = {
  bridge: GuruTerminalBridge;
};

type FreeFilter = "all" | MarketplaceFreeState;

const FREE_FILTERS: Array<{ id: FreeFilter; label: string }> = [
  { id: "all", label: "All" },
  { id: "keyless", label: "No key" },
  { id: "free_account", label: "Free account" },
  { id: "local", label: "Local" },
];

function entrySearchText(entry: MarketplaceEntry) {
  return [
    entry.name,
    entry.summary,
    entry.publisher,
    entry.data_authority,
    entry.plugin,
    ...entry.runtime.provider_ids,
    ...entry.markets,
    ...entry.asset_classes,
    ...entry.capabilities,
  ]
    .join(" ")
    .toLocaleLowerCase();
}

export function MarketplaceView({ bridge }: Props) {
  const [snapshot, setSnapshot] = useState<MarketplaceSnapshot | null>(null);
  const [query, setQuery] = useState("");
  const [freeFilter, setFreeFilter] = useState<FreeFilter>("all");
  const [error, setError] = useState<string | null>(null);
  const [reloadToken, setReloadToken] = useState(0);
  const [selectedSourceId, setSelectedSourceId] = useState("official");
  const [selectedEntryId, setSelectedEntryId] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    setError(null);
    void bridge
      .marketplaceSnapshot()
      .then((next) => {
        if (active) setSnapshot(next);
      })
      .catch(() => {
        if (!active) return;
        setError("Could not load Marketplace.");
      });
    return () => {
      active = false;
    };
  }, [bridge, reloadToken]);

  useEffect(() => {
    setSelectedEntryId(null);
  }, [reloadToken]);

  const connectorById = useMemo(
    () =>
      new Map(
        snapshot?.connectors.map((connector) => [
          connector.entry_id,
          connector,
        ]) ?? [],
      ),
    [snapshot],
  );
  const sources = snapshot?.sources ?? defaultSources();
  const selectedSource =
    sources.find((source) => source.id === selectedSourceId) ??
    sources[0] ??
    null;
  const visibleEntries = useMemo(() => {
    const normalizedQuery = query.trim().toLocaleLowerCase();
    return [...(snapshot?.catalog.entries ?? [])].filter(
      (entry) =>
        (freeFilter === "all" || entry.free_state === freeFilter) &&
        (!normalizedQuery || entrySearchText(entry).includes(normalizedQuery)),
    );
  }, [freeFilter, query, snapshot]);
  const discoveryGroups = useMemo(() => {
    if (!snapshot) return [];
    return snapshot.plugins
      .map((plugin) => ({
        id: plugin.name,
        label: plugin.interface.displayName,
        entries: visibleEntries.filter((entry) => entry.plugin === plugin.name),
      }))
      .filter((group) => group.entries.length > 0);
  }, [snapshot, visibleEntries]);
  const selectedEntry = selectedEntryId
    ? (snapshot?.catalog.entries.find(
        (entry) => entry.id === selectedEntryId,
      ) ?? null)
    : null;

  async function refreshMarketplace() {
    const next = await bridge.marketplaceSnapshot();
    setSnapshot(next);
  }

  return (
    <section className="marketplace-page" aria-labelledby="marketplace-title">
      <div className="marketplace-shell">
        <header className="marketplace-heading">
          <div>
            <h1 id="marketplace-title">Marketplace</h1>
            <p>
              Official plugins ship with the app. Keys you enter stay on this
              device.
            </p>
            {snapshot ? (
              <p>
                {snapshot.catalog.entries.length}{" "}
                {snapshot.catalog.entries.length === 1
                  ? "capability"
                  : "capabilities"}
              </p>
            ) : null}
          </div>
        </header>

        {error ? (
          <div className="marketplace-error" role="alert">
            <DatabaseIcon aria-hidden="true" />
            <div>
              <strong>Marketplace is unavailable</strong>
              <p>{error}</p>
            </div>
            <Button
              type="button"
              variant="outline"
              onClick={() => setReloadToken((current) => current + 1)}
            >
              <RefreshCwIcon /> Retry
            </Button>
          </div>
        ) : (
          <div className="marketplace-browser">
            <div
              className="marketplace-sources"
              role="tablist"
              aria-label="Marketplace sources"
            >
              {sources.map((source) => (
                <button
                  key={source.id}
                  type="button"
                  role="tab"
                  aria-selected={selectedSourceId === source.id}
                  className="marketplace-source-tab"
                  onClick={() => setSelectedSourceId(source.id)}
                >
                  {source.display_name}
                  {source.status === "coming_soon" ? (
                    <span className="marketplace-source-soon">Coming soon</span>
                  ) : null}
                </button>
              ))}
            </div>

            {selectedSource?.status === "coming_soon" ? (
              <ComingSoonSource source={selectedSource} />
            ) : (
              <>
                <div className="marketplace-controls">
                  <label className="marketplace-search">
                    <SearchIcon aria-hidden="true" />
                    <Input
                      type="search"
                      value={query}
                      aria-label="Search Marketplace"
                      placeholder="Search data sources and tools"
                      onChange={(event) => setQuery(event.target.value)}
                    />
                  </label>
                  <Select
                    value={freeFilter}
                    onValueChange={(value) =>
                      setFreeFilter(value as FreeFilter)
                    }
                  >
                    <SelectTrigger
                      aria-label="Filter tools by access"
                      className="marketplace-filter-select"
                    >
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent position="popper" align="end">
                      {FREE_FILTERS.map((filter) => (
                        <SelectItem value={filter.id} key={filter.id}>
                          {filter.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>

                {!snapshot ? (
                  <div
                    className="marketplace-loading"
                    aria-label="Loading Marketplace"
                  >
                    <span /> <span /> <span />
                  </div>
                ) : discoveryGroups.length ? (
                  <div className="marketplace-groups">
                    {discoveryGroups.map((group) => (
                      <section className="marketplace-group" key={group.id}>
                        {group.entries.length > 1 ||
                        group.entries[0]?.name !== group.label ? (
                          <h2>{group.label}</h2>
                        ) : null}
                        <div className="marketplace-grid">
                          {group.entries.map((entry) => (
                            <MarketplaceCard
                              key={entry.id}
                              entry={entry}
                              connector={connectorById.get(entry.id)}
                              onSetup={(next) => setSelectedEntryId(next.id)}
                            />
                          ))}
                        </div>
                      </section>
                    ))}
                  </div>
                ) : (
                  <div className="marketplace-empty">
                    <h2>No matching tools</h2>
                    <p>Try another search or choose All access.</p>
                  </div>
                )}
              </>
            )}
          </div>
        )}
      </div>

      <MarketplaceSetupDialog
        bridge={bridge}
        entry={selectedEntry}
        connector={
          selectedEntryId ? connectorById.get(selectedEntryId) : undefined
        }
        onClose={() => setSelectedEntryId(null)}
        onChanged={refreshMarketplace}
      />
    </section>
  );
}

function ComingSoonSource({ source }: { source: MarketplaceSource }) {
  return (
    <div className="marketplace-coming-soon">
      <h2>{source.display_name} is coming soon</h2>
      <p>{source.summary}</p>
      <p>
        {source.id === "libraries"
          ? "Later you will subscribe to Wiki and Lens packs over GitHub. Nothing is installed from this tab today."
          : "Later you will browse reviewed community plugins here. Nothing is installed from this tab today."}
      </p>
    </div>
  );
}

function defaultSources(): MarketplaceSource[] {
  return [
    {
      id: "official",
      display_name: "Guru Terminal",
      status: "ready",
      summary: "Bundled data sources and local analysis tools.",
    },
    {
      id: "community",
      display_name: "Community",
      status: "coming_soon",
      summary: "Reviewed community plugins will appear here.",
    },
    {
      id: "libraries",
      display_name: "Libraries",
      status: "coming_soon",
      summary: "Shared Wiki and Lens libraries will appear here.",
    },
  ];
}
