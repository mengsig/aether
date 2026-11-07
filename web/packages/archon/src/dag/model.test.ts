import type { ArchonGraph } from "@aether-ui/core";
import { describe, expect, it } from "vitest";
import { criticalPath, dagData, dagDisplay, dagStructure, heldUp, nodeStatus, reasonsFor } from "./model.ts";

const node = (id: string, state: string, extra: Record<string, unknown> = {}) => ({
  id, title: `Title ${id}`, pipeline: "adaptive", priority: "normal", state, spent_usd: 0, tags: [], created_at: "2026-11-04T09:00:00Z",
  updated_at: "2026-11-04T09:00:00Z", agents_running: 0, ...extra,
});
const edge = (from: string, to: string, satisfied = false, kind = "depends_on") => ({ from, to, kind, satisfied });

function graph(nodes: ReturnType<typeof node>[], edges: ReturnType<typeof edge>[], asOf = 7): ArchonGraph {
  return { nodes: Object.fromEntries(nodes.map((n) => [n.id, n])), edges, initiatives: [{ id: "in_a", title: "Alpha", state: "open", atomic: false }], asOf, truncated: false };
}

describe("DAG structure and display", () => {
  it("keys the structure on ids, groups and edges only, never on status or titles", () => {
    const before = dagData(graph([node("a", "running", { initiative: "in_a" }), node("b", "queued")], [edge("a", "b")]));
    const after = dagData(graph([node("a", "succeeded", { initiative: "in_a", title: "Renamed" }), node("b", "queued", { waiting_reason: "capacity" })], [edge("a", "b", true)], 9));
    expect(dagStructure(after, new Set()).key).toBe(dagStructure(before, new Set()).key);
    const moved = dagData(graph([node("a", "running"), node("b", "queued")], [edge("a", "b")]));
    expect(dagStructure(moved, new Set()).key).not.toBe(dagStructure(before, new Set()).key);
    const folded = dagStructure(before, new Set(["initiative:in_a", "initiative:unknown"]));
    expect([...folded.model.collapsed]).toEqual(["initiative:in_a"]);
    const display = dagDisplay(after, dagStructure(after, new Set()).model);
    expect(display.nodes.map((n) => [n.label, n.status])).toEqual([["Renamed", "succeeded"], ["Title b", "waiting"]]);
    expect(display.groups).toEqual([{ id: "initiative:in_a", label: "Alpha" }]);
  });

  it("drops edges to nodes a truncated graph does not hold", () => {
    const data = dagData(graph([node("a", "queued")], [edge("a", "zz"), edge("a", "a")]));
    expect(data.edges).toEqual([]);
  });

  it("paints the reason the operator must clear over the state", () => {
    expect(nodeStatus({ state: "running", waiting_reason: "operator_gate" })).toBe("operator");
    expect(nodeStatus({ state: "queued", waiting_reason: "dependency_failed" })).toBe("blocked");
    expect(nodeStatus({ state: "queued", waiting_reason: "capacity" })).toBe("waiting");
    expect(nodeStatus({ state: "failed", waiting_reason: null })).toBe("failed");
  });
});

describe("the critical path", () => {
  it("is the longest chain of open items, the same for any input order", () => {
    const nodes = [node("a", "succeeded"), node("b", "running"), node("c", "queued"), node("d", "queued"), node("e", "queued"), node("f", "failed")];
    const edges = [edge("a", "b", true), edge("b", "c"), edge("c", "d"), edge("b", "e"), edge("f", "e")];
    const path = criticalPath(dagData(graph(nodes, edges)));
    expect(path.nodes).toEqual(["b", "c", "d"]);
    expect([...path.emphasis.edges]).toEqual(["b\nc", "c\nd"]);
    const shuffled = criticalPath(dagData(graph([...nodes].reverse(), [...edges].reverse())));
    expect(shuffled.nodes).toEqual(path.nodes);
  });

  it("is empty without a chain, and counts items on a cycle instead of looping", () => {
    expect(criticalPath(dagData(graph([node("a", "queued"), node("b", "queued")], []))).nodes).toEqual([]);
    const cycle = criticalPath(dagData(graph([node("a", "queued"), node("b", "queued"), node("c", "queued")], [edge("a", "b"), edge("b", "a"), edge("c", "a")])));
    expect(cycle.cyclic).toBe(2);
    expect(cycle.nodes).toEqual([]);
  });
});

describe("held-up items", () => {
  it("says why each is held: its state, its waiting reason and its stuck prerequisites", () => {
    const data = dagData(graph([
      node("a", "failed"), node("b", "queued", { waiting_reason: "dependency_failed" }), node("c", "blocked", { state_detail: "Needs a decision", waiting_reason: "blocked" }),
      node("d", "queued"), node("e", "running"), node("f", "succeeded"),
    ], [edge("a", "b"), edge("e", "d"), edge("c", "d", false, "stacked_on")]));
    const held = heldUp(data);
    expect(held.map((entry) => [entry.id, entry.status])).toEqual([["b", "blocked"], ["c", "blocked"], ["d", "queued"]]);
    expect(reasonsFor(data, "b")).toEqual(["A dependency failed", "Waits on “Title a” (failed)"]);
    expect(reasonsFor(data, "c")).toEqual(["Needs a decision"]);
    expect(reasonsFor(data, "d")).toEqual(["Stacked on “Title c” (blocked)", "Waits on “Title e” (running)"]);
    expect(reasonsFor(data, "f")).toEqual([]);
  });
});
