import { routeHref } from "@aether-ui/core";
import type { GraphLegendEntry, NodeStyle } from "@aether-ui/graph";
import type { aether } from "@aether-ui/protocol";
import { EmptyState } from "@aether-ui/ui";
import { costCaveat, formatCost } from "../conversation/format.ts";
import { agentStatus, modelName, type FleetRow } from "../fleet/rows.ts";

export const AGENT_LEGEND: readonly GraphLegendEntry[] = [
  { label: "Created", status: "queued" }, { label: "Running", status: "running" },
  { label: "Waiting", status: "waiting" }, { label: "Blocked", status: "blocked" },
  { label: "Completed", status: "succeeded" }, { label: "Failed", status: "failed" },
  { label: "Cancelled", status: "cancelled" }, { label: "Delegation", tone: "muted", edge: "solid" },
  { label: "Orchestrator", tone: "muted", shape: "rounded-rect" }, { label: "Worker", tone: "muted", shape: "pill" },
  { label: "Reviewer", tone: "muted", shape: "diamond" },
];

export function agentNodeStyle(agent: aether.Agent | undefined): NodeStyle {
  if (agent === undefined) return { shape: "rounded-rect", icon: "▤" };
  const role = agent.role.toLowerCase();
  if (/review/.test(role)) return { shape: "diamond", icon: "◇" };
  if (/orchestrat|archon-run|coordinator/.test(role)) return { shape: "rounded-rect", icon: "◎" };
  return { shape: "pill", icon: "◉" };
}

export function AgentDetails({ row }: { row: FleetRow }) {
  const { agent, conn } = row;
  const usage = agent.stats?.usage;
  return <>
    <strong>{agent.name}</strong>
    <p>{agentStatus(agent).label} · {agent.role || "Agent"}</p>
    <p>{modelName(agent)}</p>
    <p title={usage === undefined ? undefined : costCaveat(usage) ?? undefined}>Cost: {usage === undefined ? "Not reported yet" : formatCost(usage)}</p>
    <a className="ui-link" href={routeHref("#/agents/:conn/:agent", { conn, agent: agent.id })}>Open agent</a>
  </>;
}

export function TopologyEmpty({ disconnected = false }: { disconnected?: boolean }) {
  return <EmptyState title={disconnected ? "No Aether daemon connected" : "No agents yet"}
    action={<a className="ui-button" data-variant="primary" href="#/activity">Start an agent</a>}>
    Agents and the agents they delegate to appear here. Start an agent from Activity.
  </EmptyState>;
}
