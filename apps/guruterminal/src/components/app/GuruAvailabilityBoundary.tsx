import { RefreshCwIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { GuruAvailability } from "../../types";

type Props = {
  availability: GuruAvailability;
  busy: boolean;
  error: string | null;
  onRecover: () => void;
  className?: string;
};

export function GuruAvailabilityBoundary({
  availability,
  busy,
  error,
  onRecover,
  className = "guru-onboarding",
}: Props) {
  if (availability.status === "available") return null;

  return (
    <section
      className={className}
      role="alert"
      aria-labelledby="guru-recovery-title"
    >
      <h1 id="guru-recovery-title">Memory needs recovery</h1>
      <p>
        An interrupted update was saved. Recover Memory to continue.
      </p>
      {error ? <div className="inline-error">{error}</div> : null}
      <div className="onboarding-actions">
        <Button type="button" disabled={busy} onClick={onRecover}>
          <RefreshCwIcon aria-hidden="true" />
          {busy ? "Recovering…" : "Recover memory"}
        </Button>
      </div>
    </section>
  );
}
