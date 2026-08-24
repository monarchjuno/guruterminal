import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import type {
  MarketplaceConnectorStatus,
  MarketplaceEntry,
  MarketplaceFreeState,
  MarketplaceTrust,
} from "../../marketplace/types";
import { CONNECTOR_READINESS_LABELS } from "../../marketplace/readiness";

export const FREE_STATE_LABELS: Record<MarketplaceFreeState, string> = {
  keyless: "No API key required",
  free_account: "Free account required",
  local: "Local data",
  paid: "Paid",
};

const TRUST_LABELS: Record<MarketplaceTrust, string> = {
  first_party: "Official",
  reviewed_community: "Reviewed",
};

export function setupActionLabel(
  entry: MarketplaceEntry,
  connector?: MarketplaceConnectorStatus,
) {
  if (entry.id === "community.web-research") return "Settings";
  if (connector?.readiness === "ready") return "Manage";
  if (
    connector?.config_state === "valid" ||
    connector?.credentials.some((credential) => credential.stored)
  ) {
    return "Continue setup";
  }
  return "Set up";
}

export function MarketplaceCard({
  entry,
  connector,
  onSetup,
}: {
  entry: MarketplaceEntry;
  connector?: MarketplaceConnectorStatus;
  onSetup?: (entry: MarketplaceEntry) => void;
}) {
  const setupAction = setupActionLabel(entry, connector);

  return (
    <Card size="sm" className="marketplace-card" data-featured={entry.featured}>
      <CardHeader>
        <div className="marketplace-entry-copy">
          <CardTitle role="heading" aria-level={3}>
            {entry.name}
          </CardTitle>
          <CardDescription>{entry.summary}</CardDescription>
          <span className="marketplace-entry-meta">
            <span>{entry.publisher}</span>
            <span>{TRUST_LABELS[entry.trust]}</span>
            <span>{FREE_STATE_LABELS[entry.free_state]}</span>
          </span>
        </div>
        <CardAction className="marketplace-entry-actions">
          {entry.release_stage === "preview" ? (
            <Badge variant="outline">Preview</Badge>
          ) : null}
          {entry.setup && onSetup ? (
            <Button
              type="button"
              size="sm"
              variant="outline"
              aria-label={`${setupAction} ${entry.name}`}
              onClick={() => onSetup(entry)}
            >
              {setupAction}
            </Button>
          ) : connector ? (
            <Badge variant="outline" data-readiness={connector.readiness}>
              {CONNECTOR_READINESS_LABELS[connector.readiness]}
            </Badge>
          ) : null}
        </CardAction>
      </CardHeader>
    </Card>
  );
}
