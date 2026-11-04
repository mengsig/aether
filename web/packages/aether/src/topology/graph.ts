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

/**
 * The runs collapsed now: those that start collapsed unless the user toggled them, others only if toggled.
 * Toggles are kept as a difference from the default, so a run that arrives later opens by its size.
 */
export function collapsedRuns(topology: Topology, toggled: ReadonlySet<string>): ReadonlySet<string> {
  const collapsed = new Set<string>();
  for (const group of topology.groups) {
    if (topology.startCollapsed.has(group.id) !== toggled.has(group.id)) collapsed.add(group.id);
  }
  invariant(collapsed.size <= topology.groups.length, "only known runs collapse");
  return collapsed;
}

/** The toggles that make every run collapsed (`collapse`) or every run expanded. */
export function toggledForAll(topology: Topology, collapse: boolean): ReadonlySet<string> {
  return new Set(topology.groups.map((group) => group.id).filter((id) => topology.startCollapsed.has(id) !== collapse));
}

export function topologyModel(topology: Topology, collapsed: ReadonlySet<string>): GraphModel {
  invariant([...collapsed].every((id) => id.startsWith("run:")), "only run groups collapse");
  return { nodes: topology.nodes, edges: topology.edges, groups: topology.groups, collapsed };
}
