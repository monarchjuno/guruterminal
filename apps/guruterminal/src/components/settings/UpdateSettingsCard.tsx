import {
  CheckCircle2Icon,
  DownloadIcon,
  RefreshCwIcon,
  ShieldAlertIcon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import type { UpdatePhase, UpdateState } from "../../types";

type Props = {
  status: UpdateState | null;
  phase: UpdatePhase;
  error: string | null;
  onCheck: () => Promise<void>;
  onInstall: (offerId: string) => Promise<void>;
};

const displayPublishedAt = (value: string) => {
  const timestamp = Date.parse(value);
  if (Number.isNaN(timestamp)) return value;
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium" }).format(
    timestamp,
  );
};

export function UpdateSettingsCard({
  status: updateState,
  phase,
  error,
  onCheck,
  onInstall,
}: Props) {
  const offer = updateState?.supported === true ? updateState.offer : null;
  const busy = phase !== "idle";
  const status =
    phase === "checking"
      ? "Checking for updates…"
      : phase === "confirming"
        ? "Waiting for your approval…"
        : phase === "downloading"
          ? updateState?.total_bytes
            ? `Downloading and verifying the update… ${Math.min(100, Math.round((updateState.downloaded_bytes / updateState.total_bytes) * 100))}%`
            : "Downloading and verifying the update…"
          : phase === "installing"
            ? "Installing the verified update…"
            : phase === "restarting"
              ? "Update installed. Restarting Guru Terminal…"
              : null;

  return (
    <Card className="update-card">
      <CardHeader>
        <CardTitle>Guru Terminal updates</CardTitle>
        <CardDescription>
          Check here for new versions. Nothing installs until you approve it.
        </CardDescription>
      </CardHeader>
      <CardContent className="update-card-content">
        <dl className="update-version-grid">
          <div>
            <dt>Current version</dt>
            <dd>{updateState?.current_version ?? "Loading…"}</dd>
          </div>
          <div>
            <dt>Available version</dt>
            <dd>{offer?.version ?? "—"}</dd>
          </div>
        </dl>

        {updateState?.supported === false ? (
          <div className="update-summary update-summary-warning">
            <ShieldAlertIcon aria-hidden="true" />
            <div>
              <strong>Automatic updates are unavailable</strong>
              <p>This development build does not install updates automatically.</p>
            </div>
          </div>
        ) : updateState && !offer ? (
          <div className="update-summary">
            <CheckCircle2Icon aria-hidden="true" />
            <div>
              <strong>Guru Terminal is up to date</strong>
              <p>No newer release is available.</p>
            </div>
          </div>
        ) : offer ? (
          <div className="update-release">
            <div className="update-summary">
              <DownloadIcon aria-hidden="true" />
              <div>
                <strong>Guru Terminal {offer.version} is available</strong>
                {offer.published_at ? (
                  <p>
                    Released{" "}
                    <time dateTime={offer.published_at}>
                      {displayPublishedAt(offer.published_at)}
                    </time>
                  </p>
                ) : null}
              </div>
            </div>
            <section aria-labelledby="update-release-notes-title">
              <h3 id="update-release-notes-title">Release notes</h3>
              <p>{offer.notes || "No release notes were provided."}</p>
            </section>
          </div>
        ) : (
          <p className="update-placeholder">
            Check for updates to see the installed and latest versions.
          </p>
        )}

        <div className="update-actions">
          <Button
            type="button"
            variant="outline"
            disabled={busy}
            onClick={() => void onCheck()}
          >
            <RefreshCwIcon
              className={phase === "checking" ? "update-spin" : undefined}
              aria-hidden="true"
            />
            {phase === "checking" ? "Checking…" : "Check for updates"}
          </Button>
          {offer ? (
            <Button
              type="button"
              disabled={busy}
              onClick={() => void onInstall(offer.offer_id)}
            >
              <DownloadIcon aria-hidden="true" />
              {phase === "confirming"
                ? "Awaiting approval…"
                : phase === "downloading" || phase === "installing"
                  ? "Installing…"
                : phase === "restarting"
                  ? "Restarting…"
                  : "Install and restart"}
            </Button>
          ) : null}
        </div>

        {status ? (
          <p className="update-progress" role="status" aria-live="polite">
            {status}
          </p>
        ) : null}
        {updateState?.blockers.length ? (
          <div className="update-summary update-summary-warning" role="status">
            <ShieldAlertIcon aria-hidden="true" />
            <div>
              <strong>Finish active work before updating</strong>
              <ul>
                {updateState.blockers.map((blocker) => (
                  <li key={`${blocker.kind}:${blocker.id}`}>{blocker.label}</li>
                ))}
              </ul>
            </div>
          </div>
        ) : null}
        {error ? (
          <p className="inline-error" role="alert">
            {error}
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}
