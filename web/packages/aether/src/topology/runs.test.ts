import { INITIAL_STATE, Store } from "@aether-ui/core";
import { describe, expect, test } from "vitest";
import { fixtureAgents, fixtureConnection, seedFleet } from "../fleet/fixture/agents.ts";
import { must } from "../fleet/fixture/must.ts";
import { selectFleetSources } from "../fleet/rows.ts";
import { buildTopology } from "./graph.ts";
import { selectRun, summarizeRuns } from "./runs.ts";

describe("topology run selection", () => {
  test("orders pending work before newer finished runs and isolates each run's nodes and edges", () => {
    const agents = fixtureAgents(20);
    for (const agent of Object.values(agents)) {
      agents[agent.id] = { ...agent, status: { state: "completed" }, updated_at: "2026-11-06T00:00:00Z" };
    }
    agents.agt_00003 = { ...must(agents.agt_00003, "agent"), status: { state: "blocked", reason: { block: "inspection", detail: "pending" } }, updated_at: "2026-11-05T00:00:00Z" };
    const store = new Store(INITIAL_STATE, () => undefined);
    seedFleet(store, fixtureConnection(), agents);
    const sources = selectFleetSources(store.get());
    const topology = buildTopology(sources);
    const runs = summarizeRuns(topology, sources);
    expect(runs[0]?.id).toContain("run_00001");
    expect(runs[0]?.active).toBe(true);
    expect(runs.slice(1).every((run) => !run.active)).toBe(true);
    const selected = selectRun(topology, must(runs[0], "run").id);
    const ids = new Set(selected.nodes.map((node) => node.id));
    expect(selected.nodes).toHaveLength(10);
    expect(selected.edges.every((edge) => ids.has(edge.from) && ids.has(edge.to))).toBe(true);
    expect(() => selectRun(topology, "missing")).toThrow();
  });
});
