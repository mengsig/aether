import type { FleetSource } from "../fleet/rows.ts";
import { fleetRows } from "../fleet/rows.ts";
import { invariant } from "../fleet/invariant.ts";
import { COLLAPSE_ABOVE, runGroupId, type Topology } from "./graph.ts";

export interface RunSummary {
  readonly id: string;
  readonly label: string;
  readonly count: number;
  readonly active: boolean;
  readonly updated: string;
}

const FINISHED = new Set(["completed", "failed", "cancelled"]);

export function summarizeRuns(topology: Topology, sources: readonly FleetSource[]): RunSummary[] {
  const stats = new Map<string, { count: number; active: boolean; updated: string }>();
  for (const { conn, agent } of fleetRows(sources)) {
    const id = runGroupId(conn, agent.run);
    const previous = stats.get(id) ?? { count: 0, active: false, updated: "" };
    stats.set(id, {
      count: previous.count + 1,
      active: previous.active || !FINISHED.has(agent.status.state),
      updated: agent.updated_at > previous.updated ? agent.updated_at : previous.updated,
    });
  }
  const runs = topology.groups.map((group) => {
    const stat = stats.get(group.id);
    invariant(stat !== undefined && stat.count > 0, `run ${group.id} has agents`);
    return { id: group.id, label: group.label, ...stat };
  });
  invariant(runs.length === stats.size, "each run has one group");
  return runs.sort((a, b) => Number(b.active) - Number(a.active) || b.updated.localeCompare(a.updated) || a.id.localeCompare(b.id));
}

export function selectRun(topology: Topology, id: string): Topology {
  const groups = topology.groups.filter((group) => group.id === id);
  const nodes = topology.nodes.filter((node) => node.group === id);
  const ids = new Set(nodes.map((node) => node.id));
  invariant(groups.length === 1 && nodes.length > 0, `selected run ${id} exists and has agents`);
  const edges = topology.edges.filter((edge) => ids.has(edge.from) && ids.has(edge.to));
  invariant(edges.length <= topology.edges.length, "run selection cannot add edges");
  const startCollapsed = new Set(nodes.length > COLLAPSE_ABOVE ? [id] : []);
  return { ...topology, groups, nodes, edges, startCollapsed };
}
