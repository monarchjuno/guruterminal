import type { MarketplaceCatalog, MarketplaceEntry } from "../marketplace/types";

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
