// The knowledge graph's model: propositions become nodes coloured by mastery, chains become edges
// between drawn nodes only, and the force layout is stretched so labelled boxes never overlap.

import { describe, expect, test } from "vitest";
import { CONCEPTS, GRAPH } from "./fixture/scenario.ts";
import { knowledgeGraph, knowledgeModel, masteryByProposition, spreadForLabels, structureKey } from "./graph-model.ts";

const graph = () => {
  const held = knowledgeGraph(GRAPH as unknown as Record<string, unknown>);
  if (held === undefined) throw new Error("the scenario has a graph");
  return held;
};

describe("knowledgeModel", () => {
  test("draws one edge per premise, skipping endpoints the reply did not carry", () => {
    const partial = { ...graph(), nodes: graph().nodes.filter((node) => node.id !== "p_uptodate") };
    const model = knowledgeModel(partial, new Map());
    expect(model.edges.some((edge) => edge.from === "p_uptodate")).toBe(false);
    expect(model.edges).toContainEqual({ from: "p_vote", to: "p_one_leader", kind: "entails" });
    expect(new Set(model.edges.map((edge) => `${edge.from}>${edge.to}`)).size).toBe(model.edges.length);
  });

  test("a mastery change keeps the structure key, so the graph is not laid out again", () => {
    const before = knowledgeModel(graph(), masteryByProposition(CONCEPTS));
    const after = knowledgeModel(graph(), new Map([["p_vote", "demonstrated"]]));
    expect(structureKey(after)).toBe(structureKey(before));
    expect(after.nodes.find((node) => node.id === "p_vote")?.status).toBe("mastery-demonstrated");
  });
});

describe("spreadForLabels", () => {
  test("stretches positions so boxes 42px apart as dots no longer overlap", () => {
    const layout = { hash: "h", positions: new Map([["a", { x: 0, y: 0 }], ["b", { x: 30, y: 29.4 }], ["c", { x: 42, y: 0 }]]) };
    const spread = spreadForLabels(layout);
    const a = spread.positions.get("a");
    const b = spread.positions.get("b");
    const c = spread.positions.get("c");
    expect(b && Math.abs(b.y - (a?.y ?? 0))).toBeGreaterThanOrEqual(44);
    expect(c && Math.abs(c.x - (a?.x ?? 0))).toBeGreaterThanOrEqual(168);
    expect(spread.hash).not.toBe(layout.hash);
  });
});
