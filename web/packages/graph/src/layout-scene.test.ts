import type { GraphModel, Layout } from "@aether-ui/protocol";
import { describe, expect, it } from "vitest";
import { seededGraph } from "../test/graphs.ts";
import { layoutInput } from "./engine.ts";
import { GROUP_HEADER, layered } from "./layered.ts";
import { visibleGraph, type VisibleGraph } from "./model.ts";
import { buildScene, contains, hitTest, overlaps } from "./scene.ts";
import { Viewport } from "./viewport.ts";

function layeredScene(model: GraphModel) {
  const graph = visibleGraph(model);
  const input = layoutInput(graph);
  const { xs, ys } = layered(input);
  const layout: Layout = { hash: graph.hash, positions: new Map(input.nodes.map((id, i) => [id, { x: xs[i] ?? NaN, y: ys[i] ?? NaN }])) };
  return { graph, scene: buildScene(graph, layout, "box") };
}

/** Every group a visible node sits in, innermost to outermost. */
function memberships(graph: VisibleGraph): Map<string, Set<string>> {
  const parents = new Map(graph.groups.map((g) => [g.id, g.parent]));
  const result = new Map<string, Set<string>>();
  for (const node of graph.nodes) {
    const inside = new Set<string>();
    for (let g = node.group; g !== undefined; g = parents.get(g)) inside.add(g);
    result.set(node.id, inside);
  }
  return result;
}

describe("layered layout in a scene", () => {
  it("never lets a group box cover a node outside the group, and its header hits the group", () => {
    const { graph, scene } = layeredScene(seededGraph(200, 400));
    const inside = memberships(graph);
    // Groups g0, g5, g10 and g15 hold no nodes in this fixture; eight of the rest are nested.
    expect(scene.groups.length).toBe(16);
    expect(scene.groups.filter((g) => g.parent !== undefined)).toHaveLength(8);
    for (const group of scene.groups) {
      for (const node of scene.index.query(group)) {
        if (!overlaps(node, { ...group, x: group.x + 1, y: group.y + 1, width: group.width - 2, height: group.height - 2 })) continue;
        expect(inside.get(node.node.id)?.has(group.id), `${node.node.id} inside ${group.id}`).toBe(true);
      }
      const header = { x: group.x + group.width / 2, y: group.y + GROUP_HEADER / 2 };
      const innermost = [...scene.groups].reverse().find((g) => contains({ ...g, height: GROUP_HEADER }, header.x, header.y));
      expect(hitTest(scene, header.x, header.y)).toEqual({ kind: "group", id: innermost?.id });
    }
  });

  it("fits whole curves, including a backward edge's bend", () => {
    const nodes = Array.from({ length: 10 }, (_, i) => ({ id: `n${i}`, label: `N${i}` }));
    const edges = [...nodes.slice(1).map((n, i) => ({ from: `n${i}`, to: n.id, kind: "x" })), { from: "n9", to: "n0", kind: "x" }];
    const { scene } = layeredScene({ nodes, edges, groups: [], collapsed: new Set() });
    const camera = new Viewport().fit(scene.bounds, 800, 600, 24);
    for (const edge of scene.edges) {
      for (const p of edge.curve) {
        const sx = p.x * camera.scale + camera.x, sy = p.y * camera.scale + camera.y;
        expect(sx >= 0 && sx <= 800 && sy >= 0 && sy <= 600, `${edge.from.node.id}>${edge.to.node.id} at ${sx},${sy}`).toBe(true);
      }
    }
  });

  it("answers a query over a vast empty area without scanning its cells", { timeout: 5_000 }, () => {
    const nodes = Array.from({ length: 2000 }, (_, i) => ({ id: `n${String(i).padStart(4, "0")}`, label: "x" }));
    const edges = nodes.slice(1).map((n, i) => ({ from: nodes[i]?.id ?? "", to: n.id, kind: "x" }));
    const { scene } = layeredScene({ nodes, edges, groups: [], collapsed: new Set() });
    expect(scene.index.query({ x: -1e7, y: -1e7, width: 2e7, height: 2e7 })).toHaveLength(2000);
    expect(scene.index.query({ x: -1e7, y: 1e6, width: 2e7, height: 1e7 })).toHaveLength(0);
  });
});
