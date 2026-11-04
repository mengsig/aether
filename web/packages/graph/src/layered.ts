// Deterministic layered (Sugiyama-style) layout for DAGs, run inside the layout worker.
// Back edges are reversed, layers come from longest paths, and barycenter sweeps order each
// layer with groups kept contiguous. Every tie breaks on node index, so output depends only on input.

import { at, invariant } from "./invariant.ts";

export interface LayoutInput {
  /** Node ids in canonical (sorted) order; positions come back in the same order. */
  nodes: readonly string[];
  /** One entry per node: its group, or "" for none. Nodes of a group stay together in a layer. */
  clusters: readonly string[];
  /** Flattened [from, to] index pairs. */
  edges: readonly number[];
}
export interface Coordinates { xs: Float64Array; ys: Float64Array }

export const LAYERED_SHAPE = { width: 168, height: 44 } as const;
const LAYER_GAP = 72;
const ROW_GAP = 20;
const SWEEPS = 8;

type Adjacency = number[][];

export function checkInput(input: LayoutInput): void {
  const n = input.nodes.length;
  invariant(input.clusters.length === n, "layout input needs one cluster per node");
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

function orderLayers(layer: Int32Array, out: Adjacency, clusters: readonly string[]): number[][] {
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
  return rows;
}

/** Centre positions, layers left to right, each layer centred on the tallest. */
export function layered(input: LayoutInput): Coordinates {
  checkInput(input);
  const n = input.nodes.length;
  const dag = breakCycles(adjacency(n, input.edges));
  const rows = orderLayers(assignLayers(dag), dag, input.clusters);
  const tallest = rows.reduce((max, row) => Math.max(max, row.length), 0);
  const xs = new Float64Array(n);
  const ys = new Float64Array(n);
  const rowPitch = LAYERED_SHAPE.height + ROW_GAP;
  rows.forEach((row, l) => {
    const offset = ((tallest - row.length) / 2) * rowPitch;
    row.forEach((v, i) => {
      xs[v] = l * (LAYERED_SHAPE.width + LAYER_GAP) + LAYERED_SHAPE.width / 2;
      ys[v] = offset + i * rowPitch + LAYERED_SHAPE.height / 2;
    });
  });
  invariant(rows.reduce((sum, row) => sum + row.length, 0) === n, "every node is placed exactly once");
  return { xs, ys };
}
