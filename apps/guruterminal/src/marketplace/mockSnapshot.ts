import marketplaceIndexJson from "../../marketplace/marketplace.json";
import type {
  GuruCapabilityBinding,
  MarketplaceCatalog,
  MarketplaceEntry,
  MarketplacePlugin,
  MarketplaceSetupField,
  MarketplaceSnapshot,
} from "./types";

type BundledConnectorEntry = Omit<MarketplaceEntry, "plugin" | "runtime"> & {
  runtime: Omit<MarketplaceEntry["runtime"], "kind"> & {
    kind: MarketplaceEntry["runtime"]["kind"] | "bundled_mcp";
  };
};

type BundledMarketplaceIndex = {
  interface: Pick<MarketplacePlugin["interface"], "displayName">;
  plugins: Array<Pick<MarketplacePlugin, "name" | "policy" | "category">>;
};

const marketplaceIndex = marketplaceIndexJson as BundledMarketplaceIndex;

const connectorModules = import.meta.glob(
  "../../marketplace/plugins/*/connectors/*.json",
  { eager: true, import: "default" },
) as Record<string, BundledConnectorEntry>;

const pluginModules = import.meta.glob(
  "../../marketplace/plugins/*/.guruterminal-plugin/plugin.json",
  { eager: true, import: "default" },
) as Record<
  string,
  {
    name: string;
    version: string;
    description: string;
    author: MarketplacePlugin["author"];
    interface: MarketplacePlugin["interface"];
  }
>;

function pluginNameFromPath(path: string, marker: string) {
  const match = path.match(new RegExp(`/plugins/([^/]+)/${marker}`));
  if (!match) {
    throw new Error(`Could not read plugin name from ${path}`);
  }
  return match[1];
}

const bundledEntries = Object.entries(connectorModules)
  .map(([path, entry]) => ({
    ...entry,
    plugin: pluginNameFromPath(path, "connectors"),
    runtime: {
      ...entry.runtime,
      kind: entry.runtime.kind === "bundled_mcp" ? "mcp" : entry.runtime.kind,
    },
    trust: "first_party" as const,
  }))
  .sort((left, right) => left.id.localeCompare(right.id));

const bundledPlugins: MarketplacePlugin[] = marketplaceIndex.plugins.map(
  (listing) => {
    const manifest = Object.entries(pluginModules).find(
      ([path]) =>
        pluginNameFromPath(path, ".guruterminal-plugin") === listing.name,
    )?.[1];
    if (!manifest) {
      throw new Error(`Missing plugin manifest for ${listing.name}`);
    }
    return {
      name: manifest.name,
      version: manifest.version,
      description: manifest.description,
      author: manifest.author,
      interface: manifest.interface,
      policy: listing.policy,
      category: listing.category,
    };
  },
);

const bundledCatalog: MarketplaceCatalog = {
  schema_version: "guruterminal-marketplace-catalog/1",
  entries: bundledEntries,
};

const connectorIds = bundledCatalog.entries
  .filter((entry) => Boolean(entry.setup))
  .map((entry) => entry.id);

const presentIds = bundledCatalog.entries.map((entry) => entry.id);

export function isValidMockSetupValue(
  field: MarketplaceSetupField,
  value: string,
) {
  if (
    value.length < field.min_length ||
    value.length > field.max_length ||
    /[\0\u0001-\u001f\u007f]/u.test(value)
  ) {
    return false;
  }
  if (field.kind === "select") return field.options.includes(value);
  if (/\s/u.test(value)) return false;
  if (field.kind === "api_key") return true;
  const parts = value.split("@");
  return parts.length === 2 && Boolean(parts[0]) && parts[1].includes(".");
}

export function createMockMarketplaceSnapshot(
  credentials = new Set<string>(),
  pendingCredentials = new Set<string>(),
  configs = new Map<string, Record<string, string>>(),
  activeCredentialFields = new Map<string, Set<string>>(),
  pendingCredentialFields = new Map<string, Set<string>>(),
  runtimeUnavailableIds = new Set<string>(),
): MarketplaceSnapshot {
  const catalog = structuredClone(bundledCatalog);
  const connectors = presentIds.map((entry_id) => {
    const entry = catalog.entries.find(
      (candidate) => candidate.id === entry_id,
    )!;
    const config = configs.get(entry_id) ?? {};
    const configFields = entry.setup?.config_fields ?? [];
    const configReady =
      Object.keys(config).every((key) =>
        configFields.some((field) => field.id === key),
      ) &&
      configFields.every((field) => {
        const value = config[field.id];
        return value === undefined
          ? !field.required
          : isValidMockSetupValue(field, value);
      });
    const declaredCredentialFields = entry.setup?.credential_fields ?? [];
    const activeFields =
      activeCredentialFields.get(entry_id) ??
      new Set(
        credentials.has(entry_id)
          ? declaredCredentialFields
              .filter((field) => field.required)
              .map((field) => field.id)
          : [],
      );
    const pendingFields =
      pendingCredentialFields.get(entry_id) ??
      new Set(
        pendingCredentials.has(entry_id)
          ? declaredCredentialFields
              .filter((field) => field.required)
              .map((field) => field.id)
          : [],
      );
    const credentialReady = declaredCredentialFields.every(
      (field) => !field.required || activeFields.has(field.id),
    );
    return {
      entry_id,
      config,
      config_state: entry.setup?.config_fields.length
        ? configReady
          ? ("valid" as const)
          : ("missing" as const)
        : ("not_required" as const),
      credentials: entry.setup?.credential_fields.length
        ? entry.setup.credential_fields.map((field) => ({
            entry_id,
            credential_id: field.id,
            stored: activeFields.has(field.id) || pendingFields.has(field.id),
            active: activeFields.has(field.id),
            pending: pendingFields.has(field.id),
            verification: activeFields.has(field.id)
              ? ("verified" as const)
              : ("never" as const),
            verified_at: activeFields.has(field.id) ? 1 : null,
            last_error: null,
          }))
        : [],
      readiness: runtimeUnavailableIds.has(entry_id)
        ? ("runtime_unavailable" as const)
        : configReady && credentialReady
          ? ("ready" as const)
          : ("needs_configuration" as const),
    };
  });

  return {
    schema_version: "guruterminal-marketplace-snapshot/1",
    sources: [
      {
        id: "official",
        display_name: marketplaceIndex.interface.displayName,
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
    ],
    plugins: structuredClone(bundledPlugins),
    catalog,
    installed: presentIds.map((entry_id) => {
      const connector = connectors.find((item) => item.entry_id === entry_id)!;
      const configured = connector.readiness === "ready";
      return {
        entry_id,
        configured,
        health: configured
          ? ("ready" as const)
          : connector.readiness === "runtime_unavailable"
            ? ("error" as const)
            : ("needs_configuration" as const),
      };
    }),
    connectors,
  };
}

export function createMockGuruCapabilityBindings(
  disabled = new Set<string>(),
  explicitlyEnabled = new Set<string>(),
  marketplace = createMockMarketplaceSnapshot(),
): GuruCapabilityBinding[] {
  return presentIds.map((entry_id) => {
    const enabled = connectorIds.includes(entry_id)
      ? explicitlyEnabled.has(entry_id)
      : !disabled.has(entry_id);
    return {
      entry_id,
      enabled,
      granted_permissions: enabled ? ["execute"] : [],
      available:
        marketplace.connectors.find((item) => item.entry_id === entry_id)
          ?.readiness === "ready",
    };
  });
}
