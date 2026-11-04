// Seeded graph fixtures for the graph package tests.

import type { GEdge, GGroup, GNode, GraphModel, Layout } from "@aether-ui/protocol";

const STATUSES = ["queued", "running", "waiting", "succeeded", "failed"];

/** A mostly forward graph of `nodes` nodes and `edges` distinct edges, with a few back edges and 20 groups. */
export function seededGraph(nodes: number, edges: number, seed = 7): GraphModel {
  let state = seed >>> 0;
  const random = () => (state = (Math.imul(1664525, state) + 1013904223) >>> 0) / 4294967296;
  const id = (i: number) => `n${String(i).padStart(5, "0")}`;
  const groups: GGroup[] = Array.from({ length: 20 }, (_, g) => ({ id: `g${g}`, label: `Group ${g}`, ...(g >= 10 ? { parent: `g${g - 10}` } : {}) }));
  const list: GNode[] = Array.from({ length: nodes }, (_, i) => ({
    id: id(i), label: `Item ${i}`, status: STATUSES[i % STATUSES.length] ?? "queued", ...(i % 5 !== 0 ? { group: `g${i % 20}` } : {}),
  }));
  const seen = new Set<string>();
  const out: GEdge[] = [];
  while (out.length < edges) {
    const a = Math.floor(random() * nodes);
    const span = 1 + Math.floor(random() * 40);
    const back = random() < 0.01;
    const b = back ? Math.max(0, a - span) : Math.min(nodes - 1, a + span);
    const key = `${a}>${b}`;
    if (a === b || seen.has(key)) continue;
    seen.add(key);
    out.push({ from: id(a), to: id(b), kind: "depends_on" });
  }
  return { nodes: list, edges: out, groups, collapsed: new Set() };
}

/** Layout positions as one string with every float's exact bytes, for byte-for-byte comparison. */
export function positionBytes(positions: ReadonlyMap<string, { x: number; y: number }>): string {
  const ids = [...positions.keys()].sort();
  const values = new Float64Array(ids.length * 2);
  ids.forEach((id, i) => { const p = positions.get(id); values[2 * i] = p?.x ?? NaN; values[2 * i + 1] = p?.y ?? NaN; });
  return `${ids.join(",")}|${Array.from(new Uint8Array(values.buffer), (b) => b.toString(16).padStart(2, "0")).join("")}`;
}

export const sceneModel: GraphModel = {
  nodes: [
    { id: "a", label: "Alpha", group: "g", status: "running" },
    { id: "b", label: "Beta", group: "g" },
    { id: "c", label: "Gamma", status: "failed" },
  ],
  edges: [{ from: "a", to: "c", kind: "x" }, { from: "b", to: "c", kind: "x" }],
  groups: [{ id: "g", label: "Group" }],
  collapsed: new Set(),
};

/** Box nodes are 168 × 44, centred on these points. */
export const sceneLayout: Layout = {
  hash: "test",
  positions: new Map([["a", { x: 100, y: 100 }], ["b", { x: 100, y: 200 }], ["c", { x: 400, y: 150 }]]),
};
