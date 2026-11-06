// GraphModel helpers: collapse state and the visible projection that layouts and the renderer
// share. A collapsed group stands in for every node below it; the outermost collapsed one wins.

import type { GGroup, GraphModel } from "@aether-ui/protocol";
import { at, invariant, must } from "./invariant.ts";

export interface VisibleNode {
  weight?: number;
  id: string;
  label: string;
  status?: string;
  /** The innermost expanded group holding this node, or undefined. */
  group?: string;
  /** Set when this node stands for a collapsed group: how many model nodes it holds. */
  collapsedMembers?: number;
}
export interface VisibleEdge { from: string; to: string; kind: string }
export interface VisibleGroup { id: string; label: string; parent?: string; depth: number }

export interface VisibleGraph {
  /** Sorted by id, so every consumer sees one order whatever the input order. */
  nodes: readonly VisibleNode[];
  edges: readonly VisibleEdge[];
  /** Expanded groups that hold at least one visible node, parents before children. */
  groups: readonly VisibleGroup[];
  /** Edges whose endpoint is not a node of the model, e.g. a truncated `task_graph`. */
  droppedEdges: number;
  /** The structure only: node ids, their groups and the edges. Status and labels are not in it. */
  key: string;
  hash: string;
}

/** A model the projection cannot interpret; the data, not the code, is wrong. */
export class GraphModelError extends Error {
  override readonly name = "GraphModelError";
}

const byString = (a: string, b: string): number => (a < b ? -1 : a > b ? 1 : 0);

export function toggleCollapsed(model: GraphModel, group: string): GraphModel {
  invariant(model.groups.some((g) => g.id === group), `toggleCollapsed: unknown group ${group}`);
  const collapsed = new Set(model.collapsed);
  if (!collapsed.delete(group)) collapsed.add(group);
  return { ...model, collapsed };
}

/** Each group's ancestor chain, innermost first, after checking ids and parents. */
function groupChains(groups: readonly GGroup[], nodeIds: ReadonlySet<string>): Map<string, readonly GGroup[]> {
  const byId = new Map<string, GGroup>();
  for (const group of groups) {
    if (byId.has(group.id)) throw new GraphModelError(`group ${group.id} appears twice`);
    if (nodeIds.has(group.id)) throw new GraphModelError(`group ${group.id} shares its id with a node`);
    byId.set(group.id, group);
  }
  const chains = new Map<string, readonly GGroup[]>();
  for (const group of groups) {
    const chain: GGroup[] = [];
    for (let at: GGroup | undefined = group; at !== undefined; at = at.parent === undefined ? undefined : byId.get(at.parent)) {
      if (chain.length > groups.length) throw new GraphModelError(`group ${group.id} is its own ancestor`);
      chain.push(at);
      if (at.parent !== undefined && !byId.has(at.parent)) throw new GraphModelError(`group ${at.id} has unknown parent ${at.parent}`);
    }
    chains.set(group.id, chain);
  }
  return chains;
}

/** The outermost collapsed group of a chain, and the innermost expanded one below it. */
function placement(chain: readonly GGroup[], collapsed: ReadonlySet<string>): { collapsed?: string; expanded?: string } {
  let outer = -1;
  chain.forEach((group, i) => { if (collapsed.has(group.id)) outer = i; });
  const below = chain[outer + 1];
  const expanded = below === undefined ? {} : { expanded: below.id };
  return outer >= 0 ? { collapsed: at(chain, outer).id, ...expanded } : expanded;
}

function nodeIdSet(model: GraphModel): Set<string> {
  const ids = new Set<string>();
  for (const node of model.nodes) {
    if (node.weight !== undefined && (!Number.isFinite(node.weight) || node.weight < 0)) throw new GraphModelError(`node ${node.id} has an invalid weight`);
    if (ids.has(node.id)) throw new GraphModelError(`node ${node.id} appears twice`);
    ids.add(node.id);
  }
  return ids;
}

/** Projects the model through its collapse state. Throws GraphModelError for inconsistent data. */
export function visibleGraph(model: GraphModel): VisibleGraph {
  const nodeIds = nodeIdSet(model);
  const chains = groupChains(model.groups, nodeIds);
  const representative = new Map<string, string>();
  const nodes = new Map<string, VisibleNode>();
  for (const node of model.nodes) {
    if (node.group !== undefined && !chains.has(node.group)) throw new GraphModelError(`node ${node.id} has unknown group ${node.group}`);
    const place = placement(node.group === undefined ? [] : must(chains, node.group), model.collapsed);
    const id = place.collapsed ?? node.id;
    representative.set(node.id, id);
    const existing = nodes.get(id);
    if (existing !== undefined) { existing.collapsedMembers = (existing.collapsedMembers ?? 0) + 1; continue; }
    const label = place.collapsed === undefined ? node.label : at(must(chains, place.collapsed), 0).label;
    nodes.set(id, {
      id, label,
      ...(place.collapsed === undefined && node.status !== undefined ? { status: node.status } : {}),
      ...(place.collapsed === undefined && node.weight !== undefined ? { weight: node.weight } : {}),
      ...(place.expanded !== undefined ? { group: place.expanded } : {}),
      ...(place.collapsed !== undefined ? { collapsedMembers: 1 } : {}),
    });
  }
  const { edges, dropped } = visibleEdges(model, representative);
  const sorted = [...nodes.values()].sort((a, b) => byString(a.id, b.id));
  const groups = visibleGroups(sorted, chains);
  invariant(sorted.length <= model.nodes.length, "a projection never adds nodes");
  // Group parents are structure too: the layered layout gives each group path its own band.
  const key = JSON.stringify([sorted.map((n) => [n.id, n.group ?? null]), groups.map((g) => [g.id, g.parent ?? null]),
    edges.map((e) => [e.from, e.to])]);
  return { nodes: sorted, edges, groups, droppedEdges: dropped, key, hash: structureHash(key) };
}

function visibleEdges(model: GraphModel, representative: ReadonlyMap<string, string>): { edges: VisibleEdge[]; dropped: number } {
  const seen = new Map<string, VisibleEdge>();
  let dropped = 0;
  for (const edge of model.edges) {
    const from = representative.get(edge.from);
    const to = representative.get(edge.to);
    if (from === undefined || to === undefined) { dropped++; continue; }
    if (from === to) continue;
    const id = JSON.stringify([from, to]);
    const kept = seen.get(id);
    if (kept === undefined || byString(edge.kind, kept.kind) < 0) seen.set(id, { from, to, kind: edge.kind });
  }
  const edges = [...seen.values()].sort((a, b) => byString(a.from, b.from) || byString(a.to, b.to));
  return { edges, dropped };
}

function visibleGroups(nodes: readonly VisibleNode[], chains: ReadonlyMap<string, readonly GGroup[]>): VisibleGroup[] {
  const shown = new Map<string, VisibleGroup>();
  for (const node of nodes) {
    if (node.group === undefined) continue;
    const chain = must(chains, node.group);
    chain.forEach((group, i) => {
      const depth = chain.length - 1 - i;
      if (!shown.has(group.id)) shown.set(group.id, { id: group.id, label: group.label, depth, ...(group.parent !== undefined ? { parent: group.parent } : {}) });
    });
  }
  return [...shown.values()].sort((a, b) => a.depth - b.depth || byString(a.id, b.id));
}

/** A 64-bit string hash (two 32-bit lanes) as 16 hex digits; cache identity uses the key itself. */
export function structureHash(key: string): string {
  let h1 = 0xdeadbeef;
  let h2 = 0x41c6ce57;
  for (let i = 0; i < key.length; i++) {
    const c = key.charCodeAt(i);
    h1 = Math.imul(h1 ^ c, 2654435761);
    h2 = Math.imul(h2 ^ c, 1597334677);
  }
  h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507) ^ Math.imul(h2 ^ (h2 >>> 13), 3266489909);
  h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507) ^ Math.imul(h1 ^ (h1 >>> 13), 3266489909);
  return (h1 >>> 0).toString(16).padStart(8, "0") + (h2 >>> 0).toString(16).padStart(8, "0");
}
