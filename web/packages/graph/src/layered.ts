// Deterministic layered (Sugiyama-style) layout for DAGs, run inside the layout worker.
// Back edges are reversed, layers come from longest paths and barycenter sweeps order each layer.
// Each group then gets its own horizontal band, so group boxes never cover another group's nodes.
// Every tie breaks on node index or group id, so output depends only on input.

import { at, invariant } from "./invariant.ts";

export interface LayoutInput {
  /** Node ids in canonical (sorted) order; positions come back in the same order. */
  nodes: readonly string[];
  /** One entry per node: its expanded groups, outermost first ([] for none). */
  groups: readonly (readonly string[])[];
  /** Flattened [from, to] index pairs. */
  edges: readonly number[];
}
export interface Coordinates { xs: Float64Array; ys: Float64Array }

export const LAYERED_SHAPE = { width: 168, height: 44 } as const;
/** A group box's padding around its contents and its label band; the scene draws the same box. */
export const GROUP_PADDING = 12;
export const GROUP_HEADER = 20;
const LAYER_GAP = 72;
const ROW_GAP = 20;
const ROW_PITCH = LAYERED_SHAPE.height + ROW_GAP;
const SWEEPS = 8;

type Adjacency = number[][];

export function checkInput(input: LayoutInput): void {
  const n = input.nodes.length;
  invariant(input.groups.length === n, "layout input needs one group path per node");
  invariant(input.edges.length % 2 === 0, "layout edges are [from, to] pairs");
  for (const end of input.edges) invariant(Number.isInteger(end) && end >= 0 && end < n, `layout edge end ${end} is out of range`);
}

const compareStrings = (a: string, b: string): number => (a < b ? -1 : a > b ? 1 : 0);

/** Forward adjacency without duplicates or self loops, neighbours ascending. */
function adjacency(n: number, edges: readonly number[]): Adjacency {
  const out: Adjacency = Array.from({ length: n }, () => []);
  for (let i = 0; i < edges.length; i += 2) {
    const from = at(edges, i);
    const to = at(edges, i + 1);
    if (from !== to) at(out, from).push(to);
  }
  return out.map((list) => [...new Set(list)].sort((a, b) => a - b));
}

/** Reverses every DFS back edge (iterative, index order), leaving a DAG. */
function breakCycles(out: Adjacency): Adjacency {
  const n = out.length;
  const state = new Uint8Array(n); // 0 new, 1 on the stack, 2 done
  const pairs: number[] = [];
  for (let root = 0; root < n; root++) {
    if (state[root] !== 0) continue;
    const stack: Array<{ node: number; next: number }> = [{ node: root, next: 0 }];
    state[root] = 1;
    for (let top = stack.at(-1); top !== undefined; top = stack.at(-1)) {
      const neighbours = at(out, top.node);
      if (top.next === neighbours.length) { state[top.node] = 2; stack.pop(); continue; }
      const v = at(neighbours, top.next++);
      if (state[v] === 1) { pairs.push(v, top.node); continue; }
      pairs.push(top.node, v);
      if (state[v] === 0) { state[v] = 1; stack.push({ node: v, next: 0 }); }
    }
  }
  return adjacency(n, pairs);
}

/** Longest-path layering by Kahn's algorithm; sources sit in layer 0. */
function assignLayers(out: Adjacency): Int32Array {
  const n = out.length;
  const indegree = new Int32Array(n);
  for (const list of out) for (const v of list) indegree[v] = at(indegree, v) + 1;
  const layer = new Int32Array(n);
  const queue: number[] = [];
  for (let v = 0; v < n; v++) if (indegree[v] === 0) queue.push(v);
  for (let head = 0; head < queue.length; head++) {
    const u = at(queue, head);
    for (const v of at(out, u)) {
      layer[v] = Math.max(at(layer, v), at(layer, u) + 1);
      indegree[v] = at(indegree, v) - 1;
      if (indegree[v] === 0) queue.push(v);
    }
  }
  invariant(queue.length === n, "layering needs an acyclic graph");
  return layer;
}

function reverse(out: Adjacency): Adjacency {
  const into: Adjacency = Array.from({ length: out.length }, () => []);
  out.forEach((list, u) => { for (const v of list) at(into, v).push(u); });
  return into;
}

function placeRow(row: readonly number[], relative: Float64Array): void {
  row.forEach((v, i) => { relative[v] = row.length === 1 ? 0.5 : i / (row.length - 1); });
}

/** One sweep step: orders `row` by its group's mean barycenter, then each node's own. */
function reorder(row: number[], neighbours: Adjacency, relative: Float64Array, clusters: readonly string[], bary: Float64Array): void {
  const clusterSum = new Map<string, { sum: number; count: number }>();
  for (const v of row) {
    const list = at(neighbours, v);
    let value = at(relative, v);
    if (list.length > 0) { value = 0; for (const u of list) value += at(relative, u); value /= list.length; }
    bary[v] = value;
    const cluster = at(clusters, v);
    if (cluster === "") continue;
    const acc = clusterSum.get(cluster) ?? { sum: 0, count: 0 };
    acc.sum += value;
    acc.count++;
    clusterSum.set(cluster, acc);
  }
  const groupValue = (v: number): number => {
    const acc = clusterSum.get(at(clusters, v));
    return acc === undefined ? at(bary, v) : acc.sum / acc.count;
  };
  const before = new Map(row.map((v, i) => [v, i]));
  const position = (v: number): number => before.get(v) ?? 0;
  row.sort((a, b) => groupValue(a) - groupValue(b) || compareStrings(at(clusters, a), at(clusters, b))
    || at(bary, a) - at(bary, b) || position(a) - position(b));
  placeRow(row, relative);
}

/** Each node's relative position (0..1) within its layer after the barycenter sweeps. */
function orderLayers(layer: Int32Array, out: Adjacency, clusters: readonly string[]): Float64Array {
  const depth = layer.reduce((max, l) => Math.max(max, l), 0) + 1;
  const rows: number[][] = Array.from({ length: depth }, () => []);
  layer.forEach((l, v) => at(rows, l).push(v));
  const into = reverse(out);
  const relative = new Float64Array(layer.length);
  const bary = new Float64Array(layer.length);
  for (const row of rows) {
    row.sort((a, b) => compareStrings(at(clusters, a), at(clusters, b)) || a - b);
    placeRow(row, relative);
  }
  for (let sweep = 0; sweep < SWEEPS; sweep++) {
    const down = sweep % 2 === 0;
    for (let k = 1; k < depth; k++) reorder(at(rows, down ? k : depth - 1 - k), down ? into : out, relative, clusters, bary);
  }
  invariant(rows.reduce((sum, row) => sum + row.length, 0) === layer.length, "every node is in exactly one layer");
  return relative;
}

/** A group's band, or the root; `direct` holds the nodes in no deeper group. */
interface Lane { id: string; group: boolean; direct: number[]; children: Map<string, Lane>; sum: number; count: number }

const newLane = (id: string, group: boolean): Lane => ({ id, group, direct: [], children: new Map(), sum: 0, count: 0 });

function laneTree(groups: readonly (readonly string[])[], relative: Float64Array): Lane {
  const root = newLane("", false);
  groups.forEach((path, v) => {
    let lane = root;
    for (const id of path) {
      let child = lane.children.get(id);
      if (child === undefined) lane.children.set(id, (child = newLane(id, true)));
      lane.sum += at(relative, v);
      lane.count++;
      lane = child;
    }
    lane.sum += at(relative, v);
    lane.count++;
    lane.direct.push(v);
  });
  return root;
}

/** Stacks a lane's direct rows and child bands top to bottom; returns the bottom edge. */
function placeLane(lane: Lane, top: number, layer: Int32Array, relative: Float64Array, ys: Float64Array): number {
  type Item = { key: string; mean: number; lane?: Lane };
  const items: Item[] = [...lane.children.values()].map((child) => ({ key: child.id, mean: child.sum / child.count, lane: child }));
  if (lane.direct.length > 0) {
    const mean = lane.direct.reduce((sum, v) => sum + at(relative, v), 0) / lane.direct.length;
    items.push({ key: "", mean });
  }
  items.sort((a, b) => a.mean - b.mean || (a.key < b.key ? -1 : a.key > b.key ? 1 : 0));
  let y = top + (lane.group ? GROUP_PADDING + GROUP_HEADER : 0);
  items.forEach((item, i) => {
    if (i > 0) y += ROW_GAP;
    y = item.lane !== undefined ? placeLane(item.lane, y, layer, relative, ys) : placeRows(lane.direct, y, layer, relative, ys);
  });
  invariant(items.length > 0 || !lane.group, `group ${lane.id} has no nodes`);
  return y + (lane.group ? GROUP_PADDING : 0);
}

/** Each layer's nodes of one band in sweep order, one row per node; returns the bottom edge. */
function placeRows(nodes: readonly number[], top: number, layer: Int32Array, relative: Float64Array, ys: Float64Array): number {
  const sorted = [...nodes].sort((a, b) => at(layer, a) - at(layer, b) || at(relative, a) - at(relative, b) || a - b);
  let rows = 0;
  let row = 0;
  sorted.forEach((v, i) => {
    row = i > 0 && at(layer, at(sorted, i - 1)) === at(layer, v) ? row + 1 : 0;
    ys[v] = top + row * ROW_PITCH + LAYERED_SHAPE.height / 2;
    rows = Math.max(rows, row + 1);
  });
  return top + rows * ROW_PITCH - ROW_GAP;
}

/** Centre positions, layers left to right, groups as nested horizontal bands. */
export function layered(input: LayoutInput): Coordinates {
  checkInput(input);
  const n = input.nodes.length;
  const clusters = input.groups.map((path) => path.at(-1) ?? "");
  const dag = breakCycles(adjacency(n, input.edges));
  const layer = assignLayers(dag);
  const relative = orderLayers(layer, dag, clusters);
  const xs = new Float64Array(n);
  const ys = new Float64Array(n);
  layer.forEach((l, v) => { xs[v] = l * (LAYERED_SHAPE.width + LAYER_GAP) + LAYERED_SHAPE.width / 2; });
  placeLane(laneTree(input.groups, relative), 0, layer, relative, ys);
  invariant(xs.every(Number.isFinite) && ys.every(Number.isFinite), "every node gets a finite position");
  return { xs, ys };
}
