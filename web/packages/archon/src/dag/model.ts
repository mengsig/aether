// What the task DAG shows, derived from core's stored `task_graph`: the structure the layout reads,
// the statuses painted over it, the critical path of open work and why items are held up.
// Structure and status are kept apart, so a status change repaints and never relayouts.

import type { ArchonGraph } from "@aether-ui/core";
import { edgeKey, type Emphasis } from "@aether-ui/graph";
import type { archon, GEdge, GGroup, GNode, GraphModel } from "@aether-ui/protocol";
import type { Status } from "@aether-ui/ui";
import { invariant, isStateKind, reasonLabel, statePill } from "../queue/model.ts";

export type TaskNode = archon.TaskNode;
export type TaskEdge = archon.TaskEdge;
export type GraphInitiative = archon.GraphInitiative;

export interface DagData {
  /** Sorted by id. */
  readonly nodes: readonly TaskNode[];
  readonly byId: ReadonlyMap<string, TaskNode>;
  /** Only edges whose two ends are nodes of this graph; a truncated graph drops the rest. */
  readonly edges: readonly TaskEdge[];
  readonly incoming: ReadonlyMap<string, readonly TaskEdge[]>;
  readonly outgoing: ReadonlyMap<string, readonly TaskEdge[]>;
  readonly initiatives: readonly GraphInitiative[];
  readonly asOf: number;
  readonly truncated: boolean;
}

/** Group ids never collide with item ids (`qi_…`). */
export const groupOf = (initiative: string): string => `initiative:${initiative}`;
export const initiativeOfGroup = (group: string): string | null => (group.startsWith("initiative:") ? group.slice("initiative:".length) : null);

const byString = (a: string, b: string): number => (a < b ? -1 : a > b ? 1 : 0);
const OPEN_EXCLUDED: ReadonlySet<string> = new Set(["succeeded", "cancelled"]);

function taskNode(value: unknown): TaskNode {
  const node = value as TaskNode;
  invariant(typeof node.id === "string" && node.id.length > 0, "a task node has an id");
  invariant(typeof node.title === "string" && isStateKind(node.state), `task node ${node.id} has a title and a known state`);
  return node;
}

function index(edges: readonly TaskEdge[], end: "from" | "to"): Map<string, TaskEdge[]> {
  const out = new Map<string, TaskEdge[]>();
  for (const edge of edges) {
    const key = edge[end];
    const list = out.get(key);
    if (list === undefined) out.set(key, [edge]); else list.push(edge);
  }
  return out;
}

/** Reads the stored graph, or a slice of it, into the shape the DAG derives from. */
export function dagData(graph: ArchonGraph, only?: (node: TaskNode) => boolean): DagData {
  const nodes = Object.values(graph.nodes).map(taskNode).filter((node) => only === undefined || only(node))
    .sort((a, b) => byString(a.id, b.id));
  const byId = new Map(nodes.map((node) => [node.id, node]));
  invariant(byId.size === nodes.length, "task graph node ids are unique");
  const edges = (graph.edges as unknown as TaskEdge[])
    .filter((edge) => edge.from !== edge.to && byId.has(edge.from) && byId.has(edge.to))
    .sort((a, b) => byString(a.from, b.from) || byString(a.to, b.to) || byString(a.kind, b.kind));
  const initiatives = graph.initiatives as unknown as GraphInitiative[];
  return { nodes, byId, edges, incoming: index(edges, "to"), outgoing: index(edges, "from"), initiatives, asOf: graph.asOf, truncated: graph.truncated };
}

/** Initiative groups that hold at least one node of `data`, by title. */
function groups(data: DagData): GGroup[] {
  const used = new Set(data.nodes.map((node) => node.initiative).filter((id): id is string => typeof id === "string"));
  const titles = new Map(data.initiatives.map((initiative) => [initiative.id, initiative.title]));
  return [...used].sort().map((id) => ({ id: groupOf(id), label: titles.get(id) ?? id }));
}

/** The structure alone: ids, groups and edges. Its key changes only when the layout must. */
export function dagStructure(data: DagData, collapsed: ReadonlySet<string>): { key: string; model: GraphModel } {
  const nodes: GNode[] = data.nodes.map((node) => ({ id: node.id, label: node.id, ...(node.initiative ? { group: groupOf(node.initiative) } : {}) }));
  const groupList = groups(data).map((group) => ({ id: group.id, label: group.id }));
  const known = new Set(groupList.map((group) => group.id));
  const folded = new Set([...collapsed].filter((id) => known.has(id)));
  const edges: GEdge[] = data.edges.map((edge) => ({ from: edge.from, to: edge.to, kind: edge.kind }));
  const key = JSON.stringify([nodes.map((node) => [node.id, node.group ?? null]), edges.map((edge) => [edge.from, edge.to, edge.kind]), [...folded].sort()]);
  invariant(nodes.length === data.nodes.length, "the structure holds every node");
  return { key, model: { nodes, edges, groups: groupList, collapsed: folded } };
}

/** The structure with titles and statuses, as the canvas paints it. */
export function dagDisplay(data: DagData, structure: GraphModel): GraphModel {
  invariant(structure.nodes.length === data.nodes.length, "the display paints the structure it was given");
  const nodes = structure.nodes.map((node) => {
    const task = data.byId.get(node.id);
    invariant(task !== undefined, `structure node ${node.id} is in the graph`);
    return { ...node, label: task.title, status: nodeStatus(task) };
  });
  const titles = new Map(groups(data).map((group) => [group.id, group.label]));
  return { ...structure, nodes, groups: structure.groups.map((group) => ({ ...group, label: titles.get(group.id) ?? group.id })) };
}

/** The status token a node is painted with: a waiting reason the operator must clear outranks its state. */
export function nodeStatus(node: Pick<TaskNode, "state" | "waiting_reason">): Status {
  const code = node.waiting_reason ?? null;
  const open = node.state === "queued" || node.state === "dispatching" || node.state === "running";
  if (open && code === "operator_gate") return "operator";
  if (open && (code === "dependency_failed" || code === "blocked")) return "blocked";
  if (node.state === "queued" && code !== null) return "waiting";
  return node.state;
}

export function statusLabel(node: Pick<TaskNode, "state" | "waiting_reason">): string {
  const status = nodeStatus(node);
  if (status === "operator") return "Needs you";
  if (status === "waiting") return "Waiting";
  invariant(isStateKind(node.state), `${node.state} is a queue state`);
  return status === "blocked" && node.state !== "blocked" ? "Held" : statePill(node.state, false).label;
}

export interface CriticalPath {
  /** Prerequisite first; empty when no chain of two or more open items exists. */
  readonly nodes: readonly string[];
  readonly emphasis: Emphasis;
  /** Open items on a dependency cycle, which no path can order. */
  readonly cyclic: number;
}

const NO_EMPHASIS: Emphasis = { nodes: new Set(), edges: new Set() };

/**
 * The longest chain of open work (not succeeded or cancelled), counted in items: what still has to finish,
 * one after another. Ties go to the smaller id, so the same graph always gives the same path.
 */
export function criticalPath(data: DagData): CriticalPath {
  const open = data.nodes.filter((node) => !OPEN_EXCLUDED.has(node.state)).map((node) => node.id);
  const isOpen = new Set(open);
  const pending = new Map(open.map((id) => [id, 0]));
  for (const edge of data.edges) if (isOpen.has(edge.from) && isOpen.has(edge.to)) pending.set(edge.to, (pending.get(edge.to) ?? 0) + 1);
  const depth = new Map<string, number>();
  const via = new Map<string, string>();
  const ready = open.filter((id) => pending.get(id) === 0);
  for (const id of ready) depth.set(id, 1);
  for (let head = 0; head < ready.length; head++) {
    const from = ready[head] as string;
    const reach = (depth.get(from) ?? 1) + 1;
    for (const edge of data.outgoing.get(from) ?? []) {
      if (!isOpen.has(edge.to)) continue;
      const best = depth.get(edge.to) ?? 0;
      const prior = via.get(edge.to);
      if (reach > best || (reach === best && prior !== undefined && from < prior)) { depth.set(edge.to, reach); via.set(edge.to, from); }
      const left = (pending.get(edge.to) ?? 0) - 1;
      pending.set(edge.to, left);
      if (left === 0) ready.push(edge.to);
    }
  }
  invariant(ready.length <= open.length, "each open item is ordered at most once");
  let end: string | undefined;
  for (const id of ready) if (end === undefined || (depth.get(id) ?? 0) > (depth.get(end) ?? 0) || ((depth.get(id) ?? 0) === (depth.get(end) ?? 0) && id < end)) end = id;
  const cyclic = open.length - ready.length;
  if (end === undefined || (depth.get(end) ?? 0) < 2) return { nodes: [], emphasis: NO_EMPHASIS, cyclic };
  const path: string[] = [];
  for (let at: string | undefined = end; at !== undefined; at = via.get(at)) path.push(at);
  path.reverse();
  invariant(path.length === depth.get(end), "the path is as long as its end's depth");
  const edges = new Set(path.slice(1).map((id, i) => edgeKey(path[i] as string, id)));
  return { nodes: path, emphasis: { nodes: new Set(path), edges }, cyclic };
}

/** Why `id` is not moving: its blocked state, its waiting reason and each prerequisite still unsatisfied. */
export function reasonsFor(data: DagData, id: string): string[] {
  const node = data.byId.get(id);
  invariant(node !== undefined, `reasons are read for a node of the graph, not ${id}`);
  if (OPEN_EXCLUDED.has(node.state) || node.state === "failed") return [];
  const reasons: string[] = [];
  if (node.state === "blocked") reasons.push(node.state_detail ? node.state_detail : "Blocked");
  // A blocked item's `blocked` waiting reason restates its state; its detail already says why.
  if (node.waiting_reason && !(node.state === "blocked" && node.waiting_reason === "blocked")) {
    const label = reasonLabel(node.waiting_reason);
    if (!reasons.includes(label)) reasons.push(label);
  }
  for (const edge of data.incoming.get(id) ?? []) {
    if (edge.satisfied) continue;
    const from = data.byId.get(edge.from);
    invariant(from !== undefined, "an indexed edge starts at a node of the graph");
    const verb = edge.kind === "stacked_on" ? "Stacked on" : edge.kind === "initiative_after" ? "After" : "Waits on";
    reasons.push(`${verb} “${from.title}” (${statusLabel(from).toLowerCase()})`);
  }
  return reasons;
}

export interface HeldEntry { readonly id: string; readonly title: string; readonly status: Status; readonly reasons: readonly string[] }

const HELD_RANK: Partial<Record<Status, number>> = { blocked: 0, operator: 1 };
const STUCK: ReadonlySet<string> = new Set(["failed", "cancelled", "blocked"]);

/** Items held up: blocked, waiting with a reason, or after a prerequisite that failed or is stuck. */
export function heldUp(data: DagData): HeldEntry[] {
  const held: HeldEntry[] = [];
  for (const node of data.nodes) {
    if (OPEN_EXCLUDED.has(node.state) || node.state === "failed") continue;
    const stuckBefore = (data.incoming.get(node.id) ?? []).some((edge) => !edge.satisfied && STUCK.has(data.byId.get(edge.from)?.state ?? ""));
    if (node.state !== "blocked" && !node.waiting_reason && !stuckBefore) continue;
    held.push({ id: node.id, title: node.title, status: nodeStatus(node), reasons: reasonsFor(data, node.id) });
  }
  invariant(held.every((entry) => entry.reasons.length > 0), "a held item says why");
  return held.sort((a, b) => (HELD_RANK[a.status] ?? 2) - (HELD_RANK[b.status] ?? 2) || a.title.localeCompare(b.title) || byString(a.id, b.id));
}

export interface InitiativeProgress {
  readonly id: string;
  readonly title: string;
  readonly state: string;
  readonly total: number;
  readonly done: number;
  readonly failed: number;
  readonly held: number;
}

/** Each initiative of the graph with how far its items in the graph have got. */
export function initiativeProgress(data: DagData, held: readonly HeldEntry[]): InitiativeProgress[] {
  const heldIds = new Set(held.map((entry) => entry.id));
  const rows = new Map(data.initiatives.map((initiative) => [initiative.id, { id: initiative.id, title: initiative.title, state: initiative.state, total: 0, done: 0, failed: 0, held: 0 }]));
  for (const node of data.nodes) {
    const row = node.initiative ? rows.get(node.initiative) : undefined;
    if (row === undefined) continue;
    row.total += 1;
    if (node.state === "succeeded") row.done += 1;
    if (node.state === "failed") row.failed += 1;
    if (heldIds.has(node.id)) row.held += 1;
  }
  return [...rows.values()].sort((a, b) => a.title.localeCompare(b.title) || byString(a.id, b.id));
}
