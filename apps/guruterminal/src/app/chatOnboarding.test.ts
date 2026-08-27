import { describe, expect, it } from "vitest";
import { createMockMarketplaceSnapshot } from "../marketplace/mockSnapshot";
import {
  isComposerMentionPlugin,
  officialSetupEntries,
} from "./chatOnboarding";

describe("composer plugin mentions", () => {
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

  it("treats EDGAR as the only config-only official setup entry", () => {
    const snapshot = createMockMarketplaceSnapshot();
    expect(officialSetupEntries(snapshot.catalog).map((entry) => entry.id)).toEqual(
      ["sec.edgar"],
    );
    expect(
      officialSetupEntries(snapshot.catalog).some(
        (entry) => entry.setup?.credential_fields.length,
      ),
    ).toBe(false);
  });
});
