import type { GraphModel } from "@aether-ui/protocol";
import { describe, expect, it } from "vitest";
import { GraphModelError, toggleCollapsed, visibleGraph } from "./model.ts";

const model: GraphModel = {
  nodes: [
    { id: "c", label: "C", group: "inner", status: "running" },
    { id: "a", label: "A", status: "queued" },
    { id: "b", label: "B", group: "outer" },
    { id: "d", label: "D", group: "inner" },
  ],
  edges: [
    { from: "a", to: "b", kind: "x" }, { from: "a", to: "c", kind: "x" }, { from: "a", to: "d", kind: "y" },
    { from: "c", to: "d", kind: "x" }, { from: "d", to: "missing", kind: "x" },
  ],
  groups: [{ id: "inner", label: "Inner", parent: "outer" }, { id: "outer", label: "Outer" }],
  collapsed: new Set(),
};

describe("visibleGraph", () => {
  it("sorts nodes and edges and reports edges to unknown nodes", () => {
    const graph = visibleGraph(model);
    expect(graph.nodes.map((n) => n.id)).toEqual(["a", "b", "c", "d"]);
    expect(graph.edges.map((e) => `${e.from}>${e.to}`)).toEqual(["a>b", "a>c", "a>d", "c>d"]);
    expect(graph.droppedEdges).toBe(1);
    expect(graph.groups.map((g) => [g.id, g.depth])).toEqual([["outer", 0], ["inner", 1]]);
  });

  it("replaces a collapsed group by one node, merging and dropping its edges", () => {
    const graph = visibleGraph(toggleCollapsed(model, "inner"));
    expect(graph.nodes.map((n) => n.id)).toEqual(["a", "b", "inner"]);
    expect(graph.nodes[2]).toEqual({ id: "inner", label: "Inner", group: "outer", collapsedMembers: 2 });
    expect(graph.edges.map((e) => `${e.from}>${e.to}:${e.kind}`)).toEqual(["a>b:x", "a>inner:x"]);
    expect(graph.groups.map((g) => g.id)).toEqual(["outer"]);
  });

  it("lets the outermost collapsed group win", () => {
    const graph = visibleGraph({ ...model, collapsed: new Set(["inner", "outer"]) });
    expect(graph.nodes.map((n) => [n.id, n.collapsedMembers])).toEqual([["a", undefined], ["outer", 3]]);
    expect(graph.groups).toEqual([]);
  });

  it("keys the structure only: status, labels, kinds and input order do not change it", () => {
    const base = visibleGraph(model);
    const restyled = visibleGraph({
      ...model,
      nodes: [...model.nodes].reverse().map((n) => ({ ...n, status: "failed", label: n.label.toLowerCase() })),
      edges: [...model.edges].reverse().map((e) => ({ ...e, kind: "other" })),
    });
    expect(restyled.key).toBe(base.key);
    expect(restyled.hash).toBe(base.hash);
    expect(base.hash).toMatch(/^[0-9a-f]{16}$/);
    expect(visibleGraph(toggleCollapsed(model, "inner")).hash).not.toBe(base.hash);
    expect(visibleGraph({ ...model, edges: model.edges.slice(1) }).hash).not.toBe(base.hash);
  });

  it("refuses inconsistent models with a GraphModelError", () => {
    const bad = (patch: Partial<GraphModel>) => () => visibleGraph({ ...model, ...patch });
    expect(bad({ nodes: [...model.nodes, { id: "a", label: "again" }] })).toThrow(GraphModelError);
    expect(bad({ groups: [...model.groups, { id: "a", label: "clash" }] })).toThrow(/shares its id/);
    expect(bad({ groups: [{ id: "inner", label: "I", parent: "inner" }, { id: "outer", label: "O" }] })).toThrow(/own ancestor/);
    expect(bad({ groups: [{ id: "inner", label: "I", parent: "nowhere" }, { id: "outer", label: "O" }] })).toThrow(/unknown parent/);
    expect(bad({ nodes: [{ id: "z", label: "Z", group: "nowhere" }] })).toThrow(/unknown group/);
  });

  it("toggles collapse without touching the input model", () => {
    const collapsed = toggleCollapsed(model, "outer");
    expect([...collapsed.collapsed]).toEqual(["outer"]);
    expect(toggleCollapsed(collapsed, "outer").collapsed.size).toBe(0);
    expect(model.collapsed.size).toBe(0);
    expect(() => toggleCollapsed(model, "nowhere")).toThrow(/unknown group/);
  });
});
