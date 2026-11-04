// The fleet as rows: every agent of every Aether connection, read through core's selectors,
// with the view's filters and the choices each filter offers. Nothing here copies daemon state.

import { selectAgents, selectConnections, type State } from "@aether-ui/core";
import type { aether, ConnId } from "@aether-ui/protocol";
import type { Status } from "@aether-ui/ui";
import { invariant } from "./invariant.ts";
import { roleLabel } from "./roles.ts";

export interface FleetSource {
  readonly conn: ConnId;
  readonly label: string;
  readonly agents: Readonly<Record<string, aether.Agent>>;
}

/** The agents of each Aether connection, in connection id order. */
export function selectFleetSources(state: State): readonly FleetSource[] {
  const sources: FleetSource[] = [];
  for (const info of Object.values(selectConnections(state))) {
    if (info.daemon !== "aether") continue;
    sources.push({ conn: info.id, label: info.label, agents: selectAgents(info.id)(state) });
  }
  return sources.sort((a, b) => compare(a.conn, b.conn));
}

/** Equal when each connection keeps its label and the very same agent map. */
export function sameSources(a: readonly FleetSource[], b: readonly FleetSource[]): boolean {
  if (a.length !== b.length) return false;
  return a.every((source, i) => {
    const other = b[i];
    return other !== undefined && other.conn === source.conn && other.label === source.label && other.agents === source.agents;
  });
}

export interface FleetRow {
  /** Unique across connections: `<conn>/<agent id>`. */
  readonly key: string;
  readonly conn: ConnId;
  readonly agent: aether.Agent;
}

const compare = (a: string, b: string): number => (a < b ? -1 : a > b ? 1 : 0);

/** One row per agent, ordered by connection and then agent id, as `list_agents_page` pages them. */
export function fleetRows(sources: readonly FleetSource[]): readonly FleetRow[] {
  const rows: FleetRow[] = [];
  for (const source of sources) {
    const ids = Object.keys(source.agents).sort(compare);
    for (const id of ids) {
      const agent = source.agents[id];
      invariant(agent !== undefined && agent.id === id, `agent ${id} is stored under its own id`);
      rows.push({ key: `${source.conn}/${id}`, conn: source.conn, agent });
    }
  }
  return rows;
}

// Aether's agent states onto the §4 status tokens; the pill keeps the agent's own word.
const STATUS: Readonly<Record<aether.AgentStatusKind, Status>> = {
  created: "queued", ready: "waiting", running: "running", waiting: "waiting", blocked: "blocked",
  completed: "succeeded", failed: "failed", cancelled: "cancelled", imported: "queued",
};

export interface AgentStatusView { readonly state: string; readonly status: Status; readonly label: string }

export function agentStatus(agent: aether.Agent): AgentStatusView {
  const state: string = agent.status.state;
  invariant(typeof state === "string" && state !== "", `agent ${agent.id} has a status state`);
  // A newer daemon may send a state this build does not know; it shows as queued under its own name.
  const status = Object.hasOwn(STATUS, state) ? STATUS[state as aether.AgentStatusKind] : "queued";
  return { state, status, label: state.charAt(0).toUpperCase() + state.slice(1) };
}

export const modelName = (agent: aether.Agent): string => `${agent.model.provider}/${agent.model.model}`;

/** Each filter is one value or "" for every value. */
export interface FleetFilter { readonly run: string; readonly role: string; readonly state: string; readonly model: string }
export const NO_FILTER: FleetFilter = Object.freeze({ run: "", role: "", state: "", model: "" });
export type FilterKey = keyof FleetFilter;
export const FILTER_KEYS: readonly FilterKey[] = ["run", "role", "state", "model"];

function valueOf(row: FleetRow, key: FilterKey): string {
  switch (key) {
    case "run": return row.agent.run;
    case "role": return row.agent.role;
    case "state": return row.agent.status.state;
    case "model": return modelName(row.agent);
  }
}

export function filterRows(rows: readonly FleetRow[], filter: FleetFilter): readonly FleetRow[] {
  const active = FILTER_KEYS.filter((key) => filter[key] !== "");
  if (active.length === 0) return rows;
  const kept = rows.filter((row) => active.every((key) => valueOf(row, key) === filter[key]));
  invariant(kept.length <= rows.length, "a filter never adds rows");
  return kept;
}

export interface FacetOption { readonly value: string; readonly label: string; readonly count: number }
export type Facets = Readonly<Record<FilterKey, readonly FacetOption[]>>;

/** A run is named after its root: the first agent of the run without a parent in it. */
export function runNames(rows: readonly FleetRow[]): ReadonlyMap<string, string> {
  const runOf = new Map(rows.map((row) => [row.key, row.agent.run]));
  const names = new Map<string, string>();
  for (const row of rows) {
    const { agent } = row;
    const parentRun = agent.parent ? runOf.get(`${row.conn}/${agent.parent}`) : undefined;
    if (parentRun === agent.run || names.has(agent.run)) continue;
    const role = roleLabel(agent.role);
    names.set(agent.run, role.kind === "archon-run" ? `${agent.name} (${role.text})` : agent.name);
  }
  return names;
}

/** The values each filter can take, with how many agents carry each, sorted by value. */
export function fleetFacets(rows: readonly FleetRow[], names: ReadonlyMap<string, string>): Facets {
  const counts = Object.fromEntries(FILTER_KEYS.map((key) => [key, new Map<string, number>()])) as Record<FilterKey, Map<string, number>>;
  for (const row of rows) {
    for (const key of FILTER_KEYS) {
      const value = valueOf(row, key);
      counts[key].set(value, (counts[key].get(value) ?? 0) + 1);
    }
  }
  const label = (key: FilterKey, value: string): string =>
    key === "run" ? names.get(value) ?? value : key === "role" ? roleLabel(value).text : value;
  const options = (key: FilterKey): FacetOption[] => [...counts[key]]
    .sort(([a], [b]) => compare(a, b))
    .map(([value, count]) => ({ value, label: label(key, value), count }));
  const facets = { run: options("run"), role: options("role"), state: options("state"), model: options("model") };
  invariant(facets.state.reduce((sum, option) => sum + option.count, 0) === rows.length, "every row has exactly one state");
  return facets;
}
