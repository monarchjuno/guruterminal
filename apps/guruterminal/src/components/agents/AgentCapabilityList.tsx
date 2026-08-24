import { useId, type ReactNode } from "react";
import { Switch } from "@/components/ui/switch";

export type AgentCapabilityItem = {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  locked?: boolean;
  note?: string;
};

type Props = {
  title: string;
  description: string;
  action?: ReactNode;
  items: AgentCapabilityItem[];
  busyId: string | null;
  emptyLabel: string;
  onToggle: (id: string) => void;
};

export function AgentCapabilityList({
  title,
  description,
  action,
  items,
  busyId,
  emptyLabel,
  onToggle,
}: Props) {
  const headingId = useId();
  const switchPrefix = useId();

  return (
    <section className="agent-section" aria-labelledby={headingId}>
      <header className="agent-section-heading">
        <div>
          <h2 id={headingId}>{title}</h2>
          <p>{description}</p>
        </div>
        {action}
      </header>

      {items.length ? (
        <div className="agent-capability-list">
          {items.map((item) => {
            const switchId = `${switchPrefix}-${item.id}`;
            return (
              <div className="agent-capability" key={item.id}>
                <label htmlFor={switchId}>
                  <strong>{item.name}</strong>
                  <span>{item.description}</span>
                  {item.note ? <small>{item.note}</small> : null}
                </label>
                <Switch
                  id={switchId}
                  checked={item.enabled}
                  disabled={item.locked || busyId !== null}
                  aria-label={`${item.name}: ${item.enabled ? "enabled" : "disabled"}`}
                  onCheckedChange={() => onToggle(item.id)}
                />
              </div>
            );
          })}
        </div>
      ) : (
        <p className="agent-capability-empty">{emptyLabel}</p>
      )}
    </section>
  );
}
