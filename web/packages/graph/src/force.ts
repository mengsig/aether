// Deterministic force layout (d3-force) for the knowledge graph, run inside the layout worker.
// A fixed seed, canonical node order and a fixed tick count make it repeatable; it never animates.

import { forceCollide, forceLink, forceManyBody, forceSimulation, forceX, forceY, type SimulationNodeDatum } from "d3-force";
import { checkInput, type Coordinates, type LayoutInput } from "./layered.ts";
import { at, invariant } from "./invariant.ts";

export const FORCE_SHAPE = { width: 28, height: 28 } as const;
const SEED = 0x2545f491;
const TICKS = 300;

/** d3's own linear congruential generator, seeded per run so no state leaks between layouts. */
function seededRandom(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (Math.imul(1664525, state) + 1013904223) >>> 0;
    return state / 4294967296;
  };
}

/** Rounds to 1/100 px so the result is stable across runs and cheap to compare. */
const round = (value: number): number => Math.round(value * 100) / 100;

export function force(input: LayoutInput): Coordinates {
  checkInput(input);
  const n = input.nodes.length;
  const nodes: SimulationNodeDatum[] = Array.from({ length: n }, (_, index) => ({ index }));
  const links: Array<{ source: number; target: number }> = [];
  for (let i = 0; i < input.edges.length; i += 2) {
    const source = at(input.edges, i);
    const target = at(input.edges, i + 1);
    if (source !== target) links.push({ source, target });
  }
  const simulation = forceSimulation(nodes)
    .randomSource(seededRandom(SEED))
    .force("charge", forceManyBody().strength(-90).distanceMax(600))
    .force("link", forceLink(links).distance(56))
    .force("collide", forceCollide(FORCE_SHAPE.width * 0.75))
    .force("x", forceX(0).strength(0.04))
    .force("y", forceY(0).strength(0.04))
    .stop();
  simulation.tick(TICKS);
  const xs = new Float64Array(n);
  const ys = new Float64Array(n);
  nodes.forEach((node, i) => {
    const { x, y } = node;
    invariant(x !== undefined && y !== undefined && Number.isFinite(x) && Number.isFinite(y), "force layout produced a non-finite position");
    xs[i] = round(x);
    ys[i] = round(y);
  });
  return { xs, ys };
}
