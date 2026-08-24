import type {
  MarketplaceConnectorStatus,
} from "./types";

export const CONNECTOR_READINESS_LABELS: Record<
  MarketplaceConnectorStatus["readiness"],
  string
> = {
  ready: "Ready",
  needs_configuration: "Needs setup",
  runtime_unavailable: "Runtime unavailable",
};

export function unavailableCapabilityNote(
  connector?: MarketplaceConnectorStatus,
): string {
  if (connector?.readiness === "runtime_unavailable") {
    return "Bundled runtime is missing from this build";
  }
  return "Set up in Marketplace";
}
