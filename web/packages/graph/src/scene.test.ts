import type { Layout } from "@aether-ui/protocol";
import { describe, expect, it } from "vitest";
import { sceneLayout, sceneModel } from "../test/graphs.ts";
import { visibleGraph } from "./model.ts";
import { buildScene, GROUP_HEADER, hitTest } from "./scene.ts";

describe("scene and hit testing", () => {
  const scene = buildScene(visibleGraph(sceneModel), sceneLayout, "box");

  it("hits the node under a point, edge inclusive, and misses empty space", () => {
    expect(hitTest(scene, 100, 100)).toEqual({ kind: "node", id: "a" });
    expect(hitTest(scene, 100 - 84, 100 - 22)).toEqual({ kind: "node", id: "a" });
    expect(hitTest(scene, 400 + 84, 150)).toEqual({ kind: "node", id: "c" });
    expect(hitTest(scene, 400 + 85, 150)).toBeUndefined();
    expect(hitTest(scene, 250, 150)).toBeUndefined();
  });

  it("hits a group's header band but not its body", () => {
    const group = scene.groups[0];
    expect(group).toMatchObject({ id: "g", x: 4, y: 100 - 22 - 12 - GROUP_HEADER });
    expect(hitTest(scene, 20, (group?.y ?? 0) + 5)).toEqual({ kind: "group", id: "g" });
    expect(hitTest(scene, 20, 150)).toBeUndefined();
  });

  it("lets the node drawn last win where nodes overlap", () => {
    const overlap: Layout = { hash: "o", positions: new Map([["a", { x: 0, y: 0 }], ["b", { x: 10, y: 0 }], ["c", { x: 500, y: 0 }]]) };
    const stacked = buildScene(visibleGraph(sceneModel), overlap, "box");
    expect(hitTest(stacked, 5, 0)).toEqual({ kind: "node", id: "b" });
  });

  it("culls in draw order, so the node drawn on top is the one a click hits", () => {
    // b's box starts in an earlier grid cell than a's, which used to reorder the query.
    const overlap: Layout = { hash: "o", positions: new Map([["a", { x: 100, y: 100 }], ["b", { x: 70, y: 100 }], ["c", { x: 900, y: 0 }]]) };
    const stacked = buildScene(visibleGraph(sceneModel), overlap, "box");
    const drawn = stacked.index.query({ x: -100, y: 0, width: 500, height: 400 }).map((n) => n.node.id);
    expect(drawn).toEqual(["a", "b"]);
    expect(hitTest(stacked, 100, 100)).toEqual({ kind: "node", id: drawn.at(-1) });
  });

  it("queries only the nodes in a rectangle and counts unplaced nodes", () => {
    expect(scene.index.query({ x: 0, y: 0, width: 200, height: 300 }).map((n) => n.node.id)).toEqual(["a", "b"]);
    const partial = buildScene(visibleGraph(sceneModel), { hash: "p", positions: new Map([["a", { x: 0, y: 0 }]]) }, "box");
    expect(partial.unplaced).toBe(2);
    expect(partial.edges).toEqual([]);
    expect(buildScene(visibleGraph(sceneModel), undefined, "dot").nodes).toEqual([]);
  });
});
