// The DAG fixture: a `task_graph` of `count` items in 40 initiatives, each item after up to three
// earlier ones nearby (so it is acyclic and layered deep), as core's store holds it.

import type { ArchonGraph } from "@aether-ui/core";
import type { archon } from "@aether-ui/protocol";
import { pad, seeded, stamp } from "./random.ts";

export const DAG_SIZE = 2_000;
const INITIATIVES = 40;

const STATES: readonly archon.QueueStateKind[] = ["queued", "queued", "running", "succeeded", "succeeded", "failed", "blocked", "dispatching"];
const KINDS: readonly archon.TaskEdgeKind[] = ["depends_on", "depends_on", "depends_on", "stacked_on", "initiative_after"];
const WAITING: readonly (archon.WaitingReasonCode | null)[] = [null, null, null, "dependency_waiting", "operator_gate", "capacity"];

export const itemId = (i: number): string => `qi_${pad(i)}`;

/** A node with every field `task_graph` sends. */
function taskNode(i: number, state: archon.QueueStateKind, initiative: string | null, waiting: archon.WaitingReasonCode | null): archon.TaskNode {
  return {
    id: itemId(i), title: `Task ${i + 1}`, state, initiative, waiting_reason: state === "queued" ? waiting : null,
    state_detail: state === "blocked" ? "waiting on review" : null, agents_running: state === "running" ? 1 : 0,
    created_at: stamp(i), updated_at: stamp(i), pipeline: "adaptive", priority: "normal", spent_usd: 0, tags: [],
    stage: state === "running" ? "build" : null,
  };
}

export function taskGraph(seed: number, count = DAG_SIZE): ArchonGraph {
  if (!Number.isInteger(count) || count < 0) throw new Error(`graph size ${count} is not a whole number`);
  const random = seeded(seed);
  const nodes: Record<string, archon.TaskNode> = {};
  const edges: archon.TaskEdge[] = [];
  for (let i = 0; i < count; i += 1) {
    const initiative = random.below(8) === 0 ? null : `in_${Math.floor((i * INITIATIVES) / Math.max(1, count))}`;
    const node = taskNode(i, random.pick(STATES), initiative, random.pick(WAITING));
    nodes[node.id] = node;
    const from = new Set<number>();
    for (let k = 0; k < 3 && i > 0; k += 1) if (random.below(10) < 7) from.add(Math.max(0, i - 1 - random.below(30)));
    for (const j of from) edges.push({ from: itemId(j), to: node.id, kind: random.pick(KINDS), satisfied: random.below(2) === 0 });
  }
  const initiatives: archon.GraphInitiative[] = Array.from({ length: INITIATIVES }, (_, n) => ({ id: `in_${n}`, title: `Initiative ${n + 1}`, state: "open", atomic: false }));
  if (Object.keys(nodes).length !== count) throw new Error("every graph node has its own id");
  return { nodes: nodes as unknown as ArchonGraph["nodes"], edges: edges as unknown as ArchonGraph["edges"], initiatives: initiatives as unknown as ArchonGraph["initiatives"], asOf: 0, truncated: false };
}

/** The same graph with one node in a new state, as core applies a `Change` (§5.3.3): structure kept. */
export function withState(graph: ArchonGraph, id: string, state: archon.QueueStateKind): ArchonGraph {
  const node = graph.nodes[id];
  if (node === undefined) throw new Error(`node ${id} is not in the graph`);
  return { ...graph, nodes: { ...graph.nodes, [id]: { ...node, state } }, asOf: graph.asOf + 1 };
}
