import { useEffect, useMemo, useRef, useState } from "react";
import {
  DownloadIcon,
  FolderOpenIcon,
  PencilIcon,
  PlusIcon,
  RefreshCwIcon,
  StoreIcon,
  Trash2Icon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type {
  GuruCapabilityBinding,
  MarketplaceSnapshot,
} from "../../marketplace/types";
import { unavailableCapabilityNote } from "../../marketplace/readiness";
import { errorMessage } from "../../errors";
import type {
  AgentSkillSummary,
  GuruTerminalBridge,
  GuruSummary,
} from "../../types";
import {
  AgentCapabilityList,
  type AgentCapabilityItem,
} from "./AgentCapabilityList";
import { AgentList } from "./AgentList";
import { GuruAvailabilityBoundary } from "../app/GuruAvailabilityBoundary";

type Props = {
  bridge: GuruTerminalBridge;
  agents: GuruSummary[];
  selectedAgent: GuruSummary | null;
  loading: boolean;
  mutationBusy: boolean;
  mutationError: string | null;
  recoveryBusy: boolean;
  recoveryError: string | null;
  onRecover: () => void;
  onSelect: (agentId: string) => void;
  onCreate: () => void;
  onImport: () => void;
  onRename: () => void;
  onExport: () => void;
  onOpenMarketplace: () => void;
  onDelete: () => Promise<void>;
  onAgentUpdated: (agent: GuruSummary) => void;
};

export function AgentsView({
  bridge,
  agents,
  selectedAgent,
  loading,
  mutationBusy,
  mutationError,
  recoveryBusy,
  recoveryError,
  onRecover,
  onSelect,
  onCreate,
  onImport,
  onRename,
  onExport,
  onOpenMarketplace,
  onDelete,
  onAgentUpdated,
}: Props) {
  const [skills, setSkills] = useState<AgentSkillSummary[]>([]);
  const [marketplace, setMarketplace] = useState<MarketplaceSnapshot | null>(
    null,
  );
  const [toolBindings, setToolBindings] = useState<GuruCapabilityBinding[]>([]);
  const [configurationError, setConfigurationError] = useState<string | null>(
    null,
  );
  const [configurationLoading, setConfigurationLoading] = useState(false);
  const [reloadToken, setReloadToken] = useState(0);
  const [busyCapabilityId, setBusyCapabilityId] = useState<string | null>(null);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const selectedAgentId = selectedAgent?.id ?? null;
  const selectedAgentIdRef = useRef(selectedAgentId);

  useEffect(() => {
    selectedAgentIdRef.current = selectedAgentId;
  }, [selectedAgentId]);

  useEffect(() => {
    let active = true;
    setSkills([]);
    setMarketplace(null);
    setToolBindings([]);
    setConfigurationError(null);
    setConfigurationLoading(Boolean(selectedAgentId));
    setBusyCapabilityId(null);
    setDeleteOpen(false);
    if (!selectedAgentId || selectedAgent?.availability.status !== "available") {
      setConfigurationLoading(false);
      return () => undefined;
    }

    void Promise.all([
      bridge.agentSkillCatalog(selectedAgentId),
      bridge.marketplaceSnapshot(),
      bridge.guruCapabilityList(selectedAgentId),
    ])
      .then(([nextSkills, nextMarketplace, nextBindings]) => {
        if (!active) return;
        setSkills(nextSkills);
        setMarketplace(nextMarketplace);
        setToolBindings(nextBindings);
      })
      .catch((cause: unknown) => {
        if (!active) return;
        setConfigurationError(
          errorMessage(cause, "Could not load this agent's tools."),
        );
      })
      .finally(() => {
        if (active) setConfigurationLoading(false);
      });

    return () => {
      active = false;
    };
  }, [bridge, reloadToken, selectedAgent?.availability.status, selectedAgentId]);

  const bindingById = useMemo(
    () =>
      new Map(
        toolBindings.map((binding) => [binding.entry_id, binding]),
      ),
    [toolBindings],
  );
  const connectorById = useMemo(
    () =>
      new Map(
        marketplace?.connectors.map((connector) => [
          connector.entry_id,
          connector,
        ]) ?? [],
      ),
    [marketplace],
  );

  const toolItems = useMemo<AgentCapabilityItem[]>(() => {
    if (!marketplace) return [];
    const entriesById = new Map(
      marketplace.catalog.entries.map((entry) => [entry.id, entry]),
    );
    return marketplace.installed.flatMap((installed) => {
      const entry = entriesById.get(installed.entry_id);
      if (!entry) {
        return [];
      }
      const binding = bindingById.get(entry.id);
      const available = binding?.available ?? false;
      return [
        {
          id: entry.id,
          name: entry.name,
          description: entry.summary,
          enabled: available && (binding?.enabled ?? false),
          locked: !available,
          note: available
            ? undefined
            : unavailableCapabilityNote(connectorById.get(entry.id)),
        },
      ];
    });
  }, [bindingById, connectorById, marketplace]);

  const skillItems = useMemo<AgentCapabilityItem[]>(
    () =>
      skills.map((skill) => ({
        id: skill.id,
        name: skill.name,
        description: skill.description,
        enabled: skill.enabled,
        locked: false,
      })),
    [skills],
  );

  const toggleSkill = async (skillId: string) => {
    if (!selectedAgent || busyCapabilityId) return;
    const operationAgentId = selectedAgent.id;
    const selected = skills.find((skill) => skill.id === skillId);
    if (!selected) return;
    const nextIds = skills.flatMap((skill) => {
      const enabled = skill.id === skillId ? !skill.enabled : skill.enabled;
      return enabled ? [skill.id] : [];
    });
    setBusyCapabilityId(skillId);
    setConfigurationError(null);
    try {
      const updated = await bridge.agentSkillsUpdate({
        guru_id: operationAgentId,
        skill_ids: nextIds,
      });
      if (selectedAgentIdRef.current !== operationAgentId) return;
      const enabledIds = new Set(updated.enabled_skill_ids);
      setSkills((current) =>
        current.map((skill) => ({
          ...skill,
          enabled: enabledIds.has(skill.id),
        })),
      );
      onAgentUpdated(updated);
    } catch (cause) {
      if (selectedAgentIdRef.current !== operationAgentId) return;
      setConfigurationError(
        errorMessage(cause, "Could not update this agent's skills."),
      );
    } finally {
      setBusyCapabilityId(null);
    }
  };

  const toggleTool = async (entryId: string) => {
    if (!selectedAgent || !marketplace || busyCapabilityId) return;
    const operationAgentId = selectedAgent.id;
    const enabled = bindingById.get(entryId)?.enabled ?? false;
    setBusyCapabilityId(entryId);
    setConfigurationError(null);
    try {
      const request = { guru_id: operationAgentId, entry_id: entryId };
      const updated = enabled
        ? await bridge.guruCapabilityDisable(request)
        : await bridge.guruCapabilityEnable(request);
      if (selectedAgentIdRef.current !== operationAgentId) return;
      setToolBindings((current) =>
        current.map((binding) =>
          binding.entry_id === entryId
            ? updated
            : binding,
        ),
      );
    } catch (cause) {
      if (selectedAgentIdRef.current !== operationAgentId) return;
      setConfigurationError(
        errorMessage(cause, "Could not update this agent's tools."),
      );
    } finally {
      setBusyCapabilityId(null);
    }
  };

  if (loading && !agents.length) {
    return (
      <section className="agents-page agents-zero" aria-label="Loading agents">
        <RefreshCwIcon className="agents-loading-icon" aria-hidden="true" />
        <p>Loading agents…</p>
      </section>
    );
  }

  if (!agents.length) {
    return (
      <section className="agents-page agents-zero" aria-labelledby="agents-title">
        <h1 id="agents-title">Create your first agent</h1>
        <p>
          Each agent keeps its own Memory, so a quality-compounder and a
          deep-value agent never contaminate each other's conclusions.
        </p>
        {mutationError ? (
          <div className="agents-zero-error" role="alert">
            {mutationError}
          </div>
        ) : null}
        <div className="agents-zero-actions">
          <Button type="button" onClick={onCreate}>
            <PlusIcon /> Create agent
          </Button>
          <Button type="button" variant="outline" onClick={onImport}>
            <FolderOpenIcon /> Import memory
          </Button>
        </div>
      </section>
    );
  }

  return (
    <section className="agents-page" aria-labelledby="agents-title">
      <header className="agents-page-heading">
        <div>
          <h1 id="agents-title">Agents</h1>
          <p>Give each agent the skills and data it needs. Memory stays separate.</p>
        </div>
        <div className="agents-page-actions">
          <Button type="button" variant="outline" onClick={onImport}>
            <FolderOpenIcon /> Import
          </Button>
          <Button type="button" onClick={onCreate}>
            <PlusIcon /> New agent
          </Button>
        </div>
      </header>

      {mutationError ? (
        <div className="inline-error" role="alert">
          {mutationError}
        </div>
      ) : null}

      <div className="agents-layout">
        <AgentList
          agents={agents}
          selectedAgentId={selectedAgent?.id ?? null}
          disabled={loading || mutationBusy}
          onSelect={onSelect}
        />

        {selectedAgent ? (
          <div className="agent-editor">
            <header className="agent-editor-heading">
              <div className="agent-identity">
                <i style={{ background: selectedAgent.accent }} aria-hidden="true" />
                <div>
                  <h2>{selectedAgent.name}</h2>
                  <p>{selectedAgent.philosophy}</p>
                </div>
              </div>
              <div className="agent-editor-actions">
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={mutationBusy}
                  onClick={onRename}
                >
                  <PencilIcon /> Rename
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={mutationBusy}
                  onClick={onExport}
                >
                  <DownloadIcon /> Export
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  className="agent-delete-button"
                  disabled={mutationBusy}
                  onClick={() => setDeleteOpen(true)}
                >
                  <Trash2Icon /> Delete
                </Button>
              </div>
            </header>

            {selectedAgent.availability.status === "recovery_required" ? (
              <GuruAvailabilityBoundary
                availability={selectedAgent.availability}
                busy={recoveryBusy}
                error={recoveryError}
                onRecover={onRecover}
                className="agents-zero"
              />
            ) : (
              <>
                {configurationError ? (
                  <div className="agent-configuration-error" role="alert">
                    <span>{configurationError}</span>
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      onClick={() => setReloadToken((current) => current + 1)}
                    >
                      <RefreshCwIcon /> Retry
                    </Button>
                  </div>
                ) : null}

                <AgentCapabilityList
                  title="Skills"
                  description="Ways of working the agent reaches for when a task fits."
                  items={skillItems}
                  busyId={busyCapabilityId}
                  emptyLabel={
                    configurationLoading
                      ? "Loading skills…"
                      : "No skills available."
                  }
                  onToggle={(id) => void toggleSkill(id)}
                />

                <AgentCapabilityList
                  title="Tools"
                  description="Which data sources this agent is allowed to reach."
                  action={
                    <Button
                      type="button"
                      size="sm"
                      variant="ghost"
                      onClick={onOpenMarketplace}
                    >
                      <StoreIcon /> Browse Marketplace
                    </Button>
                  }
                  items={toolItems}
                  busyId={busyCapabilityId}
                  emptyLabel={
                    configurationLoading
                      ? "Loading tools…"
                      : "No tools are available."
                  }
                  onToggle={(id) => void toggleTool(id)}
                />
              </>
            )}
          </div>
        ) : (
          <div className="agent-editor-empty">
            <p>Select an agent to edit it.</p>
          </div>
        )}
      </div>

      <Dialog open={deleteOpen} onOpenChange={setDeleteOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Delete agent?</DialogTitle>
            <DialogDescription>
                {selectedAgent
                ? `“${selectedAgent.name}” and its Memory and chats will be permanently deleted.`
                : "This agent will be permanently deleted."}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              disabled={mutationBusy}
              onClick={() => setDeleteOpen(false)}
            >
              Cancel
            </Button>
            <Button
              type="button"
              variant="destructive"
              disabled={mutationBusy}
              onClick={() => void onDelete()}
            >
              {mutationBusy ? "Deleting…" : "Delete agent"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
