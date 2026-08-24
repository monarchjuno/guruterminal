import type { GuruSummary } from "../../types";

type Props = {
  agents: GuruSummary[];
  selectedAgentId: string | null;
  disabled: boolean;
  onSelect: (agentId: string) => void;
};

export function AgentList({
  agents,
  selectedAgentId,
  disabled,
  onSelect,
}: Props) {
  return (
    <aside className="agents-list-pane" aria-label="Agents">
      <div className="agents-list-heading">
        <strong>Your agents</strong>
        <span>{agents.length}</span>
      </div>
      <div className="agents-list">
        {agents.map((agent) => (
          <button
            key={agent.id}
            type="button"
            className="agents-list-item"
            data-active={agent.id === selectedAgentId}
            aria-current={agent.id === selectedAgentId ? "true" : undefined}
            disabled={disabled}
            onClick={() => onSelect(agent.id)}
          >
            <i style={{ background: agent.accent }} aria-hidden="true" />
            <span>
              <strong>{agent.name}</strong>
              <small>
                {agent.availability.status === "recovery_required"
                  ? "Needs recovery"
                  : agent.record_count === 1
                    ? "1 note"
                    : `${agent.record_count} notes`}
              </small>
            </span>
          </button>
        ))}
      </div>
    </aside>
  );
}
