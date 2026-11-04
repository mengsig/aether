import { describe, expect, it } from "vitest";
import { positionBytes, seededGraph } from "../test/graphs.ts";
import { force } from "./force.ts";
import { layered, LAYERED_SHAPE, type LayoutInput } from "./layered.ts";
import { visibleGraph } from "./model.ts";

function inputOf(model: ReturnType<typeof seededGraph>): LayoutInput {
  const graph = visibleGraph(model);
  const index = new Map(graph.nodes.map((n, i) => [n.id, i]));
  return { nodes: graph.nodes.map((n) => n.id), clusters: graph.nodes.map((n) => n.group ?? ""),
    edges: graph.edges.flatMap((e) => [index.get(e.from) ?? -1, index.get(e.to) ?? -1]) };
}

function positions(input: LayoutInput, run: typeof layered) {
  const { xs, ys } = run(input);
  return new Map(input.nodes.map((id, i) => [id, { x: xs[i] ?? NaN, y: ys[i] ?? NaN }]));
}

describe("layered", () => {
  it("gives the same coordinates byte for byte, whatever the model's input order", () => {
    const model = seededGraph(300, 600);
    const shuffled = { ...model, nodes: [...model.nodes].reverse(), edges: [...model.edges].reverse() };
    const first = positionBytes(positions(inputOf(model), layered));
    expect(positionBytes(positions(inputOf(model), layered))).toBe(first);
    expect(positionBytes(positions(inputOf(shuffled), layered))).toBe(first);
  });

  it("puts every forward edge's target in a later layer and keeps nodes apart", () => {
    const input = { nodes: ["a", "b", "c", "d"], clusters: ["", "", "", ""], edges: [0, 1, 1, 2, 0, 2, 2, 3] };
    const { xs, ys } = layered(input);
    expect([...xs]).toEqual([0, 1, 2, 3].map((l) => l * (LAYERED_SHAPE.width + 72) + LAYERED_SHAPE.width / 2));
    expect(new Set(ys).size).toBe(1);
  });

  it("breaks cycles and ignores self loops and duplicate edges", () => {
    const { xs } = layered({ nodes: ["a", "b", "c"], clusters: ["", "", ""], edges: [0, 1, 1, 2, 2, 0, 1, 1, 0, 1] });
    expect(new Set(xs).size).toBe(3);
    expect(xs.every(Number.isFinite)).toBe(true);
  });

  it("keeps a group's nodes next to each other within a layer", () => {
    const { ys } = layered({ nodes: ["a", "b", "c", "d"], clusters: ["g", "", "g", ""], edges: [] });
    const order = [0, 1, 2, 3].sort((p, q) => (ys[p] ?? 0) - (ys[q] ?? 0));
    expect(Math.abs(order.indexOf(0) - order.indexOf(2))).toBe(1);
  });

  it("refuses edges that point outside the node list", () => {
    expect(() => layered({ nodes: ["a"], clusters: [""], edges: [0, 1] })).toThrow(/out of range/);
    expect(() => layered({ nodes: ["a"], clusters: [], edges: [] })).toThrow(/one cluster per node/);
  });
});

describe("force", () => {
  it("is deterministic byte for byte and finite", () => {
    const input = inputOf(seededGraph(150, 300));
    const first = positions(input, force);
    expect(positionBytes(positions(input, force))).toBe(positionBytes(first));
    expect([...first.values()].every((p) => Number.isFinite(p.x) && Number.isFinite(p.y))).toBe(true);
  });
});
