import { ExternalLinkIcon, ShieldCheckIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import type {
  MarketplaceConnectorStatus,
  MarketplaceEntry,
} from "../../marketplace/types";
import { CONNECTOR_READINESS_LABELS } from "../../marketplace/readiness";
import { errorMessage } from "../../errors";
import type { GuruTerminalBridge } from "../../types";
import { setupActionLabel } from "./MarketplaceCard";

type SetupMessage = {
  tone: "error" | "success";
  title: string;
  detail: string;
};

const VERIFICATION_LABELS: Record<
  MarketplaceConnectorStatus["credentials"][number]["verification"],
  string
> = {
  never: "Not verified",
  verified: "Verified",
  rejected: "Verification failed",
  temporarily_unavailable: "Verification unavailable",
};

function selectOptionLabel(option: string) {
  const labels: Record<string, string> = {
    real: "Live",
    demo: "Demo",
    automatic: "Automatic",
    model_only: "Model search only",
    exa_only: "Exa only",
  };
  return labels[option] ?? option.charAt(0).toUpperCase() + option.slice(1);
}

export function MarketplaceSetupDialog({
  bridge,
  entry,
  connector,
  onClose,
  onChanged,
}: {
  bridge: GuruTerminalBridge;
  entry: MarketplaceEntry | null;
  connector?: MarketplaceConnectorStatus;
  onClose: () => void;
  onChanged: () => Promise<void>;
}) {
  const [configDraft, setConfigDraft] = useState<Record<string, string>>({});
  const [credentialDraft, setCredentialDraft] = useState<
    Record<string, string>
  >({});
  const [setupBusy, setSetupBusy] = useState<"save" | "delete" | null>(null);
  const [setupMessage, setSetupMessage] = useState<SetupMessage | null>(null);
  const [credentialToDelete, setCredentialToDelete] = useState<string | null>(
    null,
  );

  const selectedEntryId = entry?.id ?? null;

  useEffect(() => {
    if (!entry) {
      setConfigDraft({});
      setCredentialDraft({});
      setSetupBusy(null);
      setSetupMessage(null);
      setCredentialToDelete(null);
      return;
    }
    setConfigDraft(
      Object.fromEntries(
        (entry.setup?.config_fields ?? []).map((field) => [
          field.id,
          connector?.config[field.id] ??
            (field.kind === "select" ? (field.options[0] ?? "") : ""),
        ]),
      ),
    );
    setCredentialDraft({});
    setSetupMessage(null);
    setCredentialToDelete(null);
    // A snapshot refresh after save must not clear the result message.
    // eslint-disable-next-line react-hooks/exhaustive-deps -- reset only when a different capability opens
  }, [selectedEntryId]);

  async function saveVerifyAndEnable() {
    if (!entry?.setup) return;
    const setup = entry.setup;
    const secretValues = { ...credentialDraft };
    setCredentialDraft({});
    setSetupBusy("save");
    setSetupMessage(null);
    try {
      const config = Object.fromEntries(
        setup.config_fields.map((field) => [
          field.id,
          (configDraft[field.id] ?? "").trim(),
        ]),
      );
      for (const field of setup.config_fields) {
        const value = config[field.id] ?? "";
        if (
          (field.required && !value) ||
          value.length < field.min_length ||
          value.length > field.max_length ||
          (field.kind === "select" && !field.options.includes(value))
        ) {
          throw new Error(`Enter a valid ${field.label.toLocaleLowerCase()}.`);
        }
      }
      const secrets = Object.fromEntries(
        setup.credential_fields.flatMap((field) => {
          const secret = (secretValues[field.id] ?? "").trim();
          return secret ? [[field.id, secret]] : [];
        }),
      );
      const patchingCredentials = Object.keys(secrets).length > 0;
      const configurationInvalidatesCredentials =
        (setup.credential_scope_fields ?? []).some(
          (fieldId) => connector?.config[fieldId] !== config[fieldId],
        ) && connector?.credentials.some((credential) => credential.stored);
      for (const field of setup.credential_fields) {
        const secret = secrets[field.id] ?? "";
        const existing = connector?.credentials.find(
          (credential) => credential.credential_id === field.id,
        );
        if (
          secret.length > 0 &&
          (secret.length < field.min_length || secret.length > field.max_length)
        ) {
          throw new Error(`Enter a valid ${field.label.toLocaleLowerCase()}.`);
        }
        if (secret.length > 0 && /\s/u.test(secret)) {
          throw new Error(`${field.label} cannot contain whitespace.`);
        }
        if (
          field.required &&
          !secret &&
          (!existing?.stored || configurationInvalidatesCredentials)
        ) {
          throw new Error(`${field.label} is required.`);
        }
      }
      if (setup.config_fields.length) {
        await bridge.marketplaceConnectorConfigure({
          entry_id: entry.id,
          config,
        });
      }
      if (patchingCredentials) {
        await bridge.marketplaceCredentialSave({
          entry_id: entry.id,
          secrets,
        });
      }
      if (setup.credential_fields.length) {
        await bridge.marketplaceCredentialVerify({
          entry_id: entry.id,
        });
      }
      await onChanged();
      setSetupMessage({
        tone: "success",
        title: setup.credential_fields.length
          ? "Verification successful"
          : "Settings saved",
        detail: `${entry.name} is ready to use.`,
      });
    } catch (cause: unknown) {
      setSetupMessage({
        tone: "error",
        title: "Verification failed",
        detail: errorMessage(cause, `Could not configure ${entry.name}.`),
      });
      void onChanged().catch(() => undefined);
    } finally {
      setCredentialDraft({});
      setSetupBusy(null);
    }
  }

  async function deleteCredential() {
    if (!entry) return;
    setCredentialDraft({});
    setSetupBusy("delete");
    setSetupMessage(null);
    try {
      await bridge.marketplaceCredentialDelete({
        entry_id: entry.id,
      });
      await onChanged();
      setSetupMessage({
        tone: "success",
        title: "Credentials deleted",
        detail: "The saved credentials were deleted from this device.",
      });
    } catch (cause: unknown) {
      setSetupMessage({
        tone: "error",
        title: "Could not delete credentials",
        detail: errorMessage(cause, "Could not delete the saved credentials."),
      });
    } finally {
      setCredentialDraft({});
      setSetupBusy(null);
      setCredentialToDelete(null);
    }
  }

  async function openSetupHelp(url: string) {
    setSetupMessage(null);
    try {
      await bridge.openExternalUrl(url);
    } catch (cause: unknown) {
      setSetupMessage({
        tone: "error",
        title: "Could not open setup guide",
        detail: errorMessage(cause, "Could not open the setup guide."),
      });
    }
  }

  return (
    <>
      <Dialog
        open={Boolean(entry)}
        onOpenChange={(open) => {
          if (!open) onClose();
        }}
      >
        {entry?.setup && (
          <DialogContent className="marketplace-setup-dialog">
            <form
              className="marketplace-setup-form"
              onSubmit={(event) => {
                event.preventDefault();
                void saveVerifyAndEnable();
              }}
            >
              <DialogHeader>
                <div className="marketplace-setup-title-row">
                  <div>
                    <DialogTitle>
                      {setupActionLabel(entry, connector)} {entry.name}
                    </DialogTitle>
                  </div>
                </div>
              </DialogHeader>

              <div className="marketplace-setup-status">
                <Badge
                  variant="outline"
                  data-readiness={connector?.readiness ?? "needs_configuration"}
                >
                  {connector
                    ? CONNECTOR_READINESS_LABELS[connector.readiness]
                    : "Needs setup"}
                </Badge>
              </div>

              <div className="marketplace-setup-fields">
                {entry.setup.config_fields.map((field) => {
                  const inputId = `marketplace-${entry.id}-${field.id}`;
                  return (
                    <div className="marketplace-setup-field" key={field.id}>
                      <div className="marketplace-setup-field-heading">
                        <Label htmlFor={inputId}>
                          {field.label}
                          {!field.required && field.kind !== "select"
                            ? " (optional)"
                            : ""}
                        </Label>
                        {field.help_url && (
                          <Button
                            type="button"
                            variant="link"
                            size="xs"
                            aria-label={`Open ${field.label} setup help`}
                            onClick={() => void openSetupHelp(field.help_url!)}
                          >
                            Setup help <ExternalLinkIcon aria-hidden="true" />
                          </Button>
                        )}
                      </div>
                      {field.kind === "select" ? (
                        <Select
                          value={
                            configDraft[field.id] ?? field.options[0] ?? ""
                          }
                          disabled={Boolean(setupBusy)}
                          onValueChange={(value) =>
                            setConfigDraft((current) => ({
                              ...current,
                              [field.id]: value,
                            }))
                          }
                        >
                          <SelectTrigger id={inputId} aria-label={field.label}>
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent position="popper">
                            {field.options.map((option) => (
                              <SelectItem value={option} key={option}>
                                {selectOptionLabel(option)}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      ) : (
                        <Input
                          id={inputId}
                          type={field.kind === "email" ? "email" : "text"}
                          value={configDraft[field.id] ?? ""}
                          required={field.required}
                          minLength={field.min_length}
                          maxLength={field.max_length}
                          disabled={Boolean(setupBusy)}
                          autoComplete={
                            field.kind === "email" ? "email" : "off"
                          }
                          onChange={(event) =>
                            setConfigDraft((current) => ({
                              ...current,
                              [field.id]: event.target.value,
                            }))
                          }
                        />
                      )}
                    </div>
                  );
                })}

                {entry.setup.credential_fields.map((field) => {
                  const inputId = `marketplace-${entry.id}-${field.id}`;
                  const status = connector?.credentials.find(
                    (credential) => credential.credential_id === field.id,
                  );
                  return (
                    <div className="marketplace-setup-field" key={field.id}>
                      <div className="marketplace-setup-field-heading">
                        <Label htmlFor={inputId}>
                          {field.label}
                          {!field.required ? " (optional)" : ""}
                        </Label>
                        {field.help_url && (
                          <Button
                            type="button"
                            variant="link"
                            size="xs"
                            aria-label={`Open ${field.label} setup help`}
                            onClick={() => void openSetupHelp(field.help_url!)}
                          >
                            Get a key <ExternalLinkIcon aria-hidden="true" />
                          </Button>
                        )}
                      </div>
                      <Input
                        id={inputId}
                        type="password"
                        value={credentialDraft[field.id] ?? ""}
                        placeholder={
                          status?.stored
                            ? "Saved securely — leave blank to keep it"
                            : `Enter ${field.label}`
                        }
                        required={field.required && !status?.stored}
                        minLength={field.min_length}
                        maxLength={field.max_length}
                        disabled={Boolean(setupBusy)}
                        autoComplete="new-password"
                        aria-describedby={`${inputId}-status`}
                        onChange={(event) =>
                          setCredentialDraft((current) => ({
                            ...current,
                            [field.id]: event.target.value,
                          }))
                        }
                      />
                      <p id={`${inputId}-status`}>
                        {status?.stored ? (
                          <>
                            Stored securely ·{" "}
                            {VERIFICATION_LABELS[status.verification]}
                          </>
                        ) : (
                          "Not stored"
                        )}
                      </p>
                    </div>
                  );
                })}
                {connector?.credentials.some(
                  (credential) => credential.stored,
                ) && (
                  <Button
                    type="button"
                    variant="destructive"
                    size="sm"
                    disabled={Boolean(setupBusy)}
                    onClick={() => setCredentialToDelete(entry.id)}
                  >
                    Delete saved credentials
                  </Button>
                )}
              </div>

              {entry.id === "koreainvestment.market-data" && (
                <div className="marketplace-credential-note">
                  <ShieldCheckIcon aria-hidden="true" />
                  <p>
                    Account lookups need your account number and product code.
                    Those stay on this device and are never sent in chat.
                  </p>
                </div>
              )}

              {entry.id === "community.web-research" && (
                <div className="marketplace-credential-note">
                  <ShieldCheckIcon aria-hidden="true" />
                  <p>
                    Automatic uses reviewed native search for OpenAI and
                    Anthropic, then falls back to Exa. xAI and other providers
                    use Exa directly. Model search only forces the current
                    provider&apos;s native path. Exa only always skips native
                    search.
                  </p>
                </div>
              )}

              {entry.setup.credential_fields.length > 0 && (
                <div className="marketplace-credential-note">
                  <ShieldCheckIcon aria-hidden="true" />
                  <p>
                    Keys are stored securely on this device and never shown
                    again.
                  </p>
                </div>
              )}

              {setupMessage && (
                <div
                  className="marketplace-setup-message"
                  data-tone={setupMessage.tone}
                  role={setupMessage.tone === "error" ? "alert" : "status"}
                >
                  <strong>{setupMessage.title}</strong>
                  <p>{setupMessage.detail}</p>
                </div>
              )}

              <DialogFooter>
                <Button type="submit" disabled={Boolean(setupBusy)}>
                  {setupBusy === "save"
                    ? "Checking…"
                    : entry.setup.credential_fields.length
                      ? "Save & verify"
                      : "Save setup"}
                </Button>
              </DialogFooter>
            </form>
          </DialogContent>
        )}
      </Dialog>
      <Dialog
        open={Boolean(credentialToDelete)}
        onOpenChange={(open) => {
          if (!open) setCredentialToDelete(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Delete saved credentials?</DialogTitle>
            <DialogDescription>
              This removes the saved keys for {entry?.name ?? "this tool"} from
              every agent that uses them. You will need to enter them again.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              disabled={setupBusy === "delete"}
              onClick={() => setCredentialToDelete(null)}
            >
              Cancel
            </Button>
            <Button
              type="button"
              variant="destructive"
              disabled={setupBusy === "delete"}
              onClick={() => {
                if (credentialToDelete) void deleteCredential();
              }}
            >
              {setupBusy === "delete" ? "Deleting…" : "Delete credentials"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
