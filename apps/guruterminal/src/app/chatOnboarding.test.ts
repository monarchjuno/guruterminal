import { describe, expect, it } from "vitest";
import { createMockGuruCapabilityBindings, createMockMarketplaceSnapshot } from "../marketplace/mockSnapshot";
import {
  emptyChatSetupSources,
  isComposerMentionPlugin,
  officialSetupEntries,
  shouldShowEmptySetup,
} from "./chatOnboarding";

describe("empty Chat setup sources", () => {
  it("keeps always-on built-in tools out of the composer @ picker", () => {
    const snapshot = createMockMarketplaceSnapshot();
    const mentionable = snapshot.catalog.entries
      .filter(isComposerMentionPlugin)
      .map((entry) => entry.id);
    expect(mentionable).toEqual(
      expect.arrayContaining([
        "sec.edgar",
        "opendart.disclosures",
        "krx.market-data",
        "fred.macro",
        "koreainvestment.market-data",
        "alpha-vantage.market-data",
        "fmp.market-data",
      ]),
    );
    const alwaysOn = snapshot.catalog.entries
      .filter((entry) => !isComposerMentionPlugin(entry))
      .map((entry) => entry.id);
    expect(alwaysOn).toEqual(expect.arrayContaining([
      "guruterminal.compute-python",
      "guruterminal.finance-core",
      "world-bank.indicators",
      "openbb.platform",
      "community.web-research",
    ]));
  });

  it("offers only the config-only EDGAR contact email on empty Chat", () => {
    const snapshot = createMockMarketplaceSnapshot();
    const sources = emptyChatSetupSources(
      snapshot,
      createMockGuruCapabilityBindings(),
    );

    expect(officialSetupEntries(snapshot.catalog).map((entry) => entry.id)).toEqual(
      ["sec.edgar"],
    );
    expect(sources).toEqual([
      expect.objectContaining({
        id: "sec.edgar",
        status: "needs_setup",
        detail: "Needs a contact email",
        emailField: { id: "contact_email", label: "SEC contact email" },
      }),
    ]);
    expect(
      officialSetupEntries(snapshot.catalog).some(
        (entry) => entry.setup?.credential_fields.length,
      ),
    ).toBe(false);
    expect(shouldShowEmptySetup(sources)).toBe(true);
  });

  it("hides the empty-chat setup once EDGAR is enabled for this Guru", () => {
    const snapshot = createMockMarketplaceSnapshot(
      new Set(),
      new Set(),
      new Map([["sec.edgar", { contact_email: "research@example.com" }]]),
    );
    const sources = emptyChatSetupSources(
      snapshot,
      createMockGuruCapabilityBindings(
        new Set(),
        new Set(["sec.edgar"]),
        snapshot,
      ),
    );

    expect(sources.find((source) => source.id === "sec.edgar")).toMatchObject({
      status: "ready",
      detail: "On for this Guru",
    });
    expect(shouldShowEmptySetup(sources)).toBe(false);
  });

  it("explains a missing bundled runtime instead of asking for setup", () => {
    const snapshot = createMockMarketplaceSnapshot(
      new Set(),
      new Set(),
      new Map(),
      new Map(),
      new Map(),
      new Set(["sec.edgar"]),
    );
    const sources = emptyChatSetupSources(
      snapshot,
      createMockGuruCapabilityBindings(new Set(), new Set(), snapshot),
    );

    expect(sources.find((source) => source.id === "sec.edgar")).toMatchObject({
      status: "needs_setup",
      detail: "Bundled runtime is missing from this build",
    });
  });

  it("asks to enable a configured source on this Guru", () => {
    const snapshot = createMockMarketplaceSnapshot(
      new Set(),
      new Set(),
      new Map([["sec.edgar", { contact_email: "research@example.com" }]]),
    );
    const sources = emptyChatSetupSources(
      snapshot,
      createMockGuruCapabilityBindings(new Set(), new Set(), snapshot),
    );

    expect(sources.find((source) => source.id === "sec.edgar")).toMatchObject({
      status: "needs_enable",
      detail: "Enable for this Guru",
      emailField: { id: "contact_email", label: "SEC contact email" },
    });
    expect(shouldShowEmptySetup(sources)).toBe(true);
  });
});
