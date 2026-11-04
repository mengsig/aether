import { INITIAL_STATE, Store } from "@aether-ui/core";
import { visibleGraph } from "@aether-ui/graph";
import { describe, expect, test } from "vitest";
import { fixtureAgents, fixtureConnection, seedFleet } from "../fleet/fixture/agents.ts";
import { must } from "../fleet/fixture/must.ts";
import { selectFleetSources } from "../fleet/rows.ts";
import { buildTopology, COLLAPSE_ABOVE, collapsedRuns, EXPANDED_LIMIT, nodeId, overridesForAll, runGroupId, toggleRun, topologyModel } from "./graph.ts";

function topologyOf(...fleets: Array<[string, number]>) {
  const store = new Store(INITIAL_STATE, () => undefined);
  for (const [conn, count] of fleets) seedFleet(store, fixtureConnection(conn, conn), fixtureAgents(count));
  return buildTopology(selectFleetSources(store.get()));
}

describe("buildTopology", () => {
  test("has a node per agent, an edge per delegation and a group per run", () => {
    const topology = topologyOf(["aether:a", 20]);
    expect(topology.nodes).toHaveLength(20);
    expect(topology.groups.map((group) => group.id)).toEqual(["run:aether:a/run_00000", "run:aether:a/run_00001", "run:aether:a/run_00002"]);
    expect(topology.edges).toHaveLength(20 - 3);
    expect(topology.edges).toContainEqual({ from: nodeId("aether:a", "agt_00000"), to: nodeId("aether:a", "agt_00001"), kind: "delegates" });
    expect(topology.nodes[0]).toEqual({ id: "aether:a/agt_00000", label: "qi-00000 · Archon run · adaptive", group: runGroupId("aether:a", "run_00000"), status: "running" });
  });

  test("keeps agents of two daemons apart, even with the same ids", () => {
    const topology = topologyOf(["aether:a", 5], ["aether:b", 5]);
    expect(new Set(topology.nodes.map((node) => node.id)).size).toBe(10);
    expect(topology.agentOf.get("aether:b/agt_00004")).toEqual({ conn: "aether:b", agent: "agt_00004" });
    expect(topology.edges.every((edge) => edge.from.slice(0, 8) === edge.to.slice(0, 8))).toBe(true);
  });

  test("names each daemon's run after its own root when run ids match", () => {
    const store = new Store(INITIAL_STATE, () => undefined);
    for (const [conn, root] of [["aether:a", "root-a"], ["aether:b", "root-b"]] as const) {
      const agents = fixtureAgents(3);
      agents.agt_00000 = { ...must(agents.agt_00000, "agt_00000"), name: root };
      seedFleet(store, fixtureConnection(conn, conn), agents);
    }
    const groups = buildTopology(selectFleetSources(store.get())).groups;
    expect(groups.map((group) => [group.id, group.label])).toEqual([
      ["run:aether:a/run_00000", "Run root-a (Archon run · adaptive)"],
      ["run:aether:b/run_00000", "Run root-b (Archon run · adaptive)"],
    ]);
  });

  test("leaves an agent whose parent is not in the snapshot a root", () => {
    const store = new Store(INITIAL_STATE, () => undefined);
    const agents = fixtureAgents(4);
    delete agents.agt_00000;
    seedFleet(store, fixtureConnection(), agents);
    expect(buildTopology(selectFleetSources(store.get())).edges).toEqual([]);
  });
});

describe("collapse per run", () => {
  const topology = topologyOf(["aether:a", 20]);
  const big = { ...topology, startCollapsed: new Set([runGroupId("aether:a", "run_00001")]) };

  test("starts with large runs collapsed, and every run in a crowded fleet", () => {
    expect(COLLAPSE_ABOVE).toBeGreaterThan(12);
    expect(collapsedRuns(topology, new Map())).toEqual(new Set());
    expect(collapsedRuns(big, new Map())).toEqual(new Set(["run:aether:a/run_00001"]));
    const crowded = topologyOf(["aether:a", EXPANDED_LIMIT + 1]);
    expect(collapsedRuns(crowded, new Map()).size).toBe(crowded.groups.length);
    expect(collapsedRuns(topologyOf(["aether:a", EXPANDED_LIMIT]), new Map()).size).toBe(0);
  });

  test("records a toggle as an explicit choice, and collapses or expands every run", () => {
    const one = toggleRun(big, new Map(), "run:aether:a/run_00001");
    expect(one).toEqual(new Map([["run:aether:a/run_00001", false]]));
    expect(collapsedRuns(big, toggleRun(big, one, "run:aether:a/run_00000"))).toEqual(new Set(["run:aether:a/run_00000"]));
    expect(collapsedRuns(big, overridesForAll(big, true)).size).toBe(3);
    expect(collapsedRuns(big, overridesForAll(big, false)).size).toBe(0);
  });

  test("keeps the user's choices when the fleet grows past EXPANDED_LIMIT", () => {
    const before = topologyOf(["aether:a", EXPANDED_LIMIT]);
    const allCollapsed = overridesForAll(before, true);
    const after = topologyOf(["aether:a", EXPANDED_LIMIT + 1]);
    expect(after.groups.length).toBeGreaterThanOrEqual(before.groups.length);
    const collapsed = collapsedRuns(after, allCollapsed);
    expect(before.groups.every((group) => collapsed.has(group.id))).toBe(true);
    const expandedByHand = new Map([["run:aether:a/run_00000", false]]);
    expect(collapsedRuns(after, expandedByHand).has("run:aether:a/run_00000")).toBe(false);
    expect(collapsedRuns(after, expandedByHand).has("run:aether:a/run_00001")).toBe(true);
  });

  test("keeps a run the user collapsed collapsed when it grows past COLLAPSE_ABOVE", () => {
    const small = topologyOf(["aether:a", 20]);
    const chosen = toggleRun(small, new Map(), "run:aether:a/run_00001");
    expect(collapsedRuns(small, chosen).has("run:aether:a/run_00001")).toBe(true);
    const grown = { ...small, startCollapsed: new Set(["run:aether:a/run_00001"]) };
    expect(collapsedRuns(grown, chosen).has("run:aether:a/run_00001")).toBe(true);
    expect(collapsedRuns(grown, toggleRun(grown, chosen, "run:aether:a/run_00001")).has("run:aether:a/run_00001")).toBe(false);
  });

  test("shows a collapsed run as one node standing for its members", () => {
    const graph = visibleGraph(topologyModel(topology, new Set(["run:aether:a/run_00001"])));
    expect(graph.nodes).toHaveLength(20 - 10 + 1);
    expect(graph.nodes.find((node) => node.id === "run:aether:a/run_00001")?.collapsedMembers).toBe(10);
  });
});
