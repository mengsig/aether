import { INITIAL_STATE, Store } from "@aether-ui/core";
import type { aether, ConnectionInfo } from "@aether-ui/protocol";
import { describe, expect, test } from "vitest";
import { fixtureAgents, fixtureConnection, seedFleet } from "./fixture/agents.ts";
import { must } from "./fixture/must.ts";
import {
  agentStatus, filterRows, fleetFacets, fleetRows, NO_FILTER, runNames, sameSources, selectFleetSources,
} from "./rows.ts";

const archond: ConnectionInfo = { id: "archon:studio", daemon: "archon", state: "ready", features: [], label: "studio" };

function twoDaemons() {
  const store = new Store(INITIAL_STATE, () => undefined);
  seedFleet(store, fixtureConnection("aether:b", "b"), fixtureAgents(20));
  seedFleet(store, fixtureConnection("aether:a", "a"), fixtureAgents(5));
  store.update((state) => ({ ...state, connections: { ...state.connections, [archond.id]: archond } }));
  return store;
}

describe("fleet sources and rows", () => {
  test("reads every Aether connection, never archond, in connection order", () => {
    const sources = selectFleetSources(twoDaemons().get());
    expect(sources.map((source) => source.conn)).toEqual(["aether:a", "aether:b"]);
    expect(Object.keys(sources[1]?.agents ?? {})).toHaveLength(20);
  });

  test("are equal while each connection keeps the same agent map", () => {
    const store = twoDaemons();
    const before = selectFleetSources(store.get());
    expect(sameSources(before, selectFleetSources(store.get()))).toBe(true);
    store.updateAether("aether:a", (state) => ({ ...state, agents: { ...state.agents } }));
    expect(sameSources(before, selectFleetSources(store.get()))).toBe(false);
  });

  test("lists each agent once, by connection and then id, with a unique key", () => {
    const rows = fleetRows(selectFleetSources(twoDaemons().get()));
    expect(rows).toHaveLength(25);
    expect(new Set(rows.map((row) => row.key)).size).toBe(25);
    expect(rows.slice(0, 2).map((row) => row.key)).toEqual(["aether:a/agt_00000", "aether:a/agt_00001"]);
    expect(rows[5]?.key).toBe("aether:b/agt_00000");
  });
});

describe("agentStatus", () => {
  const agent = fixtureAgents(1).agt_00000 as aether.Agent;

  test("maps every Aether state onto a status token and keeps its word", () => {
    expect(agentStatus({ ...agent, status: { state: "completed" } })).toEqual({ state: "completed", status: "succeeded", label: "Completed" });
    expect(agentStatus({ ...agent, status: { state: "ready" } }).status).toBe("waiting");
    expect(agentStatus({ ...agent, status: { state: "failed", error: "x" } }).status).toBe("failed");
  });

  test("shows a state from a newer daemon under its own name", () => {
    const future = { ...agent, status: { state: "hibernating" } } as unknown as aether.Agent;
    expect(agentStatus(future)).toEqual({ state: "hibernating", status: "queued", label: "Hibernating" });
  });
});

describe("filters and facets", () => {
  const rows = fleetRows(selectFleetSources(twoDaemons().get()));

  test("keep only rows matching every active filter", () => {
    expect(filterRows(rows, NO_FILTER)).toBe(rows);
    const running = filterRows(rows, { ...NO_FILTER, state: "running" });
    expect(running.length).toBeGreaterThan(0);
    expect(running.every((row) => row.agent.status.state === "running")).toBe(true);
    const narrow = filterRows(rows, { ...NO_FILTER, run: "aether:a/run_00000", role: "archon-stage:adaptive/build" });
    expect(narrow.map((row) => row.key)).toEqual(["aether:a/agt_00002"]);
    expect(filterRows(rows, { ...NO_FILTER, model: "codex/gpt-6-astra", run: "aether:a/run_00001" }).every((row) => row.agent.model.provider === "codex")).toBe(true);
  });

  test("offer each value with its count, and name runs after their root", () => {
    const names = runNames(rows);
    expect(names.get("aether:a/run_00000")).toBe("qi-00000 (Archon run · adaptive)");
    expect(names.get("aether:b/run_00001")).toBe("run-00001");
    const facets = fleetFacets(rows, names);
    expect(facets.role.find((option) => option.value === "archon-run:adaptive")).toEqual({ value: "archon-run:adaptive", label: "Archon run · adaptive", count: 2 });
    expect(facets.model.reduce((sum, option) => sum + option.count, 0)).toBe(rows.length);
    expect(facets.run.map((option) => option.value)).toEqual([...facets.run.map((option) => option.value)].sort());
  });
});

describe("values that look alike", () => {
  test("selects agents with no role without meaning every role", () => {
    const store = new Store(INITIAL_STATE, () => undefined);
    const agents = fixtureAgents(2);
    agents.agt_00000 = { ...must(agents.agt_00000, "agt_00000"), role: "" };
    seedFleet(store, fixtureConnection(), agents);
    const rows = fleetRows(selectFleetSources(store.get()));
    const noRole = must(fleetFacets(rows, runNames(rows)).role.find((option) => option.label === "No role"), "the No role option");
    expect(noRole).toEqual({ value: "", label: "No role", count: 1 });
    expect(filterRows(rows, { ...NO_FILTER, role: noRole.value }).map((row) => row.agent.id)).toEqual(["agt_00000"]);
    expect(filterRows(rows, NO_FILTER)).toHaveLength(2);
  });

  test("keeps runs of two daemons apart when their ids match", () => {
    const store = new Store(INITIAL_STATE, () => undefined);
    for (const [conn, root] of [["aether:a", "root-a"], ["aether:b", "root-b"]] as const) {
      const agents = fixtureAgents(3);
      agents.agt_00000 = { ...must(agents.agt_00000, "agt_00000"), name: root };
      seedFleet(store, fixtureConnection(conn, conn), agents);
    }
    const rows = fleetRows(selectFleetSources(store.get()));
    const names = runNames(rows);
    expect([names.get("aether:a/run_00000"), names.get("aether:b/run_00000")]).toEqual(["root-a (Archon run · adaptive)", "root-b (Archon run · adaptive)"]);
    const runs = fleetFacets(rows, names).run;
    expect(runs.map((option) => [option.value, option.count])).toEqual([["aether:a/run_00000", 3], ["aether:b/run_00000", 3]]);
    expect(filterRows(rows, { ...NO_FILTER, run: "aether:b/run_00000" }).every((row) => row.conn === "aether:b")).toBe(true);
  });
});
