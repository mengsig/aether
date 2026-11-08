// The knowledge graph's model: propositions become nodes coloured by mastery, chains become edges
// between drawn nodes only, each titled so a person can read it.

import { describe, expect, test } from "vitest";
import { CONCEPTS, GRAPH } from "./fixture/scenario.ts";
import { knowledgeGraph, knowledgeModel, masteryByProposition, readableTitle, structureKey } from "./graph-model.ts";

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

describe("readableTitle", () => {
  test("a slug name reads as words, a spaced name stays as written", () => {
    expect(readableTitle({ name: "raft-leader_election", statement: "Raft elects one leader per term." })).toBe("Raft leader election");
    expect(readableTitle({ name: "Quorum overlap", statement: "Any two majorities share a node." })).toBe("Quorum overlap");
  });

  test("without a name the statement's first clause titles it, cut to a title's length", () => {
    expect(readableTitle({ name: "", statement: "A term has at most one leader; votes are per term." })).toBe("A term has at most one leader");
    const long = readableTitle({ name: " ", statement: `${"word ".repeat(30)}end.` });
    expect(long.length).toBeLessThanOrEqual(60);
    expect(long.endsWith("…")).toBe(true);
  });

  test("the graph's nodes carry the readable title", () => {
    const named = { ...graph(), nodes: graph().nodes.map((node, index) => (index === 0 ? { ...node, name: "log-matching" } : node)) };
    expect(knowledgeModel(named, new Map()).nodes[0]?.label).toBe("Log matching");
  });
});
