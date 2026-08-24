import { unavailableCapabilityNote } from "../marketplace/readiness";
import type { GuruCapabilityBinding } from "../marketplace/types";
import type {
  MarketplaceCatalog,
  MarketplaceEntry,
  MarketplaceSnapshot,
} from "../marketplace/types";

type EmptySetupStatus = "needs_setup" | "needs_enable" | "ready";

type EmptySetupEmailField = {
  id: string;
  label: string;
};

export type EmptySetupSource = {
  id: string;
  name: string;
  status: EmptySetupStatus;
  detail: string;
  emailField: EmptySetupEmailField | null;
};

/** Built-in always-on tools stay out of the composer `@` / `/` plugin picker. */
export const isComposerMentionPlugin = (entry: MarketplaceEntry): boolean =>
  Boolean(entry.setup) && entry.plugin !== "web-research";

export const officialSetupEntries = (
  catalog: MarketplaceCatalog,
): MarketplaceEntry[] =>
  catalog.entries.filter(
    (entry) =>
      entry.featured &&
      entry.runtime.kind === "mcp" &&
      Boolean(entry.setup) &&
      (entry.setup?.credential_fields.length ?? 0) === 0,
  );

const setupNeedDetail = (entry: MarketplaceEntry): string => {
  if (entry.setup?.config_fields.some((field) => field.kind === "email")) {
    return "Needs a contact email";
  }
  if (entry.setup?.credential_fields.some((field) => field.required)) {
    return "Needs an API key";
  }
  if (entry.setup?.config_fields.length) {
    return "Needs a one-time setup";
  }
  return "Set up in Marketplace";
};

export const emptyChatSetupSources = (
  snapshot: MarketplaceSnapshot,
  bindings: ReadonlyArray<GuruCapabilityBinding>,
): EmptySetupSource[] => {
  const bindingById = new Map(
    bindings.map((binding) => [binding.entry_id, binding]),
  );
  const connectorById = new Map(
    snapshot.connectors.map((connector) => [connector.entry_id, connector]),
  );
  return officialSetupEntries(snapshot.catalog).map((entry) => {
    const binding = bindingById.get(entry.id);
    const connector = connectorById.get(entry.id);
    const ready = connector?.readiness === "ready";
    const enabled = Boolean(binding?.enabled && binding.available);
    const emailField = entry.setup?.config_fields.find(
      (field) => field.kind === "email",
    );
    const email = emailField
      ? { id: emailField.id, label: emailField.label }
      : null;
    if (enabled) {
      return {
        id: entry.id,
        name: entry.name,
        status: "ready",
        detail: "On for this Guru",
        emailField: email,
      };
    }
    if (ready) {
      return {
        id: entry.id,
        name: entry.name,
        status: "needs_enable",
        detail: "Enable for this Guru",
        emailField: email,
      };
    }
    return {
      id: entry.id,
      name: entry.name,
      status: "needs_setup",
      detail:
        connector?.readiness === "runtime_unavailable"
          ? unavailableCapabilityNote(connector)
          : setupNeedDetail(entry),
      emailField: email,
    };
  });
};

export const shouldShowEmptySetup = (
  sources: ReadonlyArray<EmptySetupSource>,
): boolean =>
  sources.length > 0 && sources.every((source) => source.status !== "ready");
