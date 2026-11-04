// The delegation topology as a GraphModel: one node per agent, an edge from each parent to its
// child, and one group per run, which collapses into a single node.

import type { ConnId, GEdge, GGroup, GNode, GraphModel } from "@aether-ui/protocol";
import { invariant } from "../fleet/invariant.ts";
import { roleLabel } from "../fleet/roles.ts";
import { agentStatus, fleetRows, runNames, type FleetSource } from "../fleet/rows.ts";

/** A run with more agents than this starts collapsed, so a large run opens readable. */
export const COLLAPSE_ABOVE = 30;
/** A fleet with more agents than this starts with every run collapsed, one node per run. */
export const EXPANDED_LIMIT = 400;

export interface AgentRef { readonly conn: ConnId; readonly agent: string }

export interface Topology {
  /** Every node and group, with nothing collapsed; `collapsedRuns` decides that. */
  readonly nodes: readonly GNode[];
  readonly edges: readonly GEdge[];
  readonly groups: readonly GGroup[];
  readonly agentOf: ReadonlyMap<string, AgentRef>;
  /** Runs that start collapsed: those above COLLAPSE_ABOVE agents, or all of them above EXPANDED_LIMIT. */
  readonly startCollapsed: ReadonlySet<string>;
}

export const nodeId = (conn: ConnId, agent: string): string => `${conn}/${agent}`;
export const runGroupId = (conn: ConnId, run: string): string => `run:${conn}/${run}`;

export function buildTopology(sources: readonly FleetSource[]): Topology {
  const rows = fleetRows(sources);
  const names = runNames(rows);
  const nodes: GNode[] = [];
  const edges: GEdge[] = [];
  const agentOf = new Map<string, AgentRef>();
  const members = new Map<string, number>();
  const groups = new Map<string, GGroup>();
  for (const { conn, agent } of rows) {
    const id = nodeId(conn, agent.id);
    const group = runGroupId(conn, agent.run);
    if (!groups.has(group)) groups.set(group, { id: group, label: `Run ${names.get(agent.run) ?? agent.run}` });
    members.set(group, (members.get(group) ?? 0) + 1);
    nodes.push({ id, label: `${agent.name} · ${roleLabel(agent.role).text}`, group, status: agentStatus(agent).status });
    agentOf.set(id, { conn, agent: agent.id });
  }
  for (const { conn, agent } of rows) {
    // A parent outside the snapshot (not yet paged in, or forgotten) leaves its child a root.
    if (agent.parent && agentOf.has(nodeId(conn, agent.parent))) {
      edges.push({ from: nodeId(conn, agent.parent), to: nodeId(conn, agent.id), kind: "delegates" });
    }
  }
  const crowded = nodes.length > EXPANDED_LIMIT;
  const startCollapsed = new Set([...members].filter(([, count]) => crowded || count > COLLAPSE_ABOVE).map(([group]) => group));
  invariant(agentOf.size === nodes.length, "every agent is one node");
  invariant(edges.length < Math.max(1, nodes.length), "a delegation tree has fewer edges than nodes");
  return { nodes, edges, groups: [...groups.values()], agentOf, startCollapsed };
}

/** The user's explicit choice per run: true collapsed, false expanded. Untouched runs keep their default. */
export type RunOverrides = ReadonlyMap<string, boolean>;

/** A choice outlives a change of default, so a run the user expanded stays open as the fleet grows. */
export function isCollapsed(topology: Topology, overrides: RunOverrides, group: string): boolean {
  return overrides.get(group) ?? topology.startCollapsed.has(group);
}

export function collapsedRuns(topology: Topology, overrides: RunOverrides): ReadonlySet<string> {
  const collapsed = new Set(topology.groups.map((group) => group.id).filter((id) => isCollapsed(topology, overrides, id)));
  invariant(collapsed.size <= topology.groups.length, "only known runs collapse");
  return collapsed;
}

/** Flips one run, recording the choice explicitly. */
export function toggleRun(topology: Topology, overrides: RunOverrides, group: string): RunOverrides {
  invariant(topology.groups.some((g) => g.id === group), `toggleRun: unknown run group ${group}`);
  return new Map(overrides).set(group, !isCollapsed(topology, overrides, group));
}

/** Every current run collapsed (`collapse`) or expanded; runs that arrive later open by their default. */
export function overridesForAll(topology: Topology, collapse: boolean): RunOverrides {
  return new Map(topology.groups.map((group) => [group.id, collapse]));
}

export function topologyModel(topology: Topology, collapsed: ReadonlySet<string>): GraphModel {
  invariant([...collapsed].every((id) => id.startsWith("run:")), "only run groups collapse");
  return { nodes: topology.nodes, edges: topology.edges, groups: topology.groups, collapsed };
}
