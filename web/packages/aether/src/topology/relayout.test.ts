// CONTRACTS.md §5.4: the topology lays out again only on structural events. Store updates
// stand in for daemon events; the counting layout function records every layout asked for.

import { INITIAL_STATE, Store } from "@aether-ui/core";
import type { aether, GraphModel, Layout } from "@aether-ui/protocol";
import { describe, expect, test } from "vitest";
import { fixtureAgents, fixtureConnection, FIXTURE_CONN, seedFleet } from "../fleet/fixture/agents.ts";
import { must } from "../fleet/fixture/must.ts";
import { selectFleetSources } from "../fleet/rows.ts";
import { buildTopology, collapsedRuns, topologyModel } from "./graph.ts";
import { LayoutGate } from "./relayout.ts";

const placeAll = async (model: GraphModel): Promise<Layout> =>
  ({ hash: "test", positions: new Map(model.nodes.map((node, i) => [node.id, { x: i, y: 0 }])) });

function scenario() {
  const store = new Store(INITIAL_STATE, () => undefined);
  seedFleet(store, fixtureConnection(), fixtureAgents(300));
  const gate = new LayoutGate(placeAll);
  let toggled: ReadonlySet<string> = new Set();
  const relayout = async () => {
    const topology = buildTopology(selectFleetSources(store.get()));
    await gate.layout(topologyModel(topology, collapsedRuns(topology, toggled)));
    return gate.requests;
  };
  const patch = (id: string, change: (agent: aether.Agent) => aether.Agent) =>
    store.updateAether(FIXTURE_CONN, (state) => ({ ...state, agents: { ...state.agents, [id]: change(must(state.agents[id], id)) } }));
  const add = (agent: aether.Agent) => store.updateAether(FIXTURE_CONN, (state) => ({ ...state, agents: { ...state.agents, [agent.id]: agent } }));
  return { store, relayout, patch, add, toggle: (next: ReadonlySet<string>) => { toggled = next; } };
}

const STATES: aether.AgentStatus[] = [{ state: "running" }, { state: "completed" }, { state: "failed", error: "x" }, { state: "ready" }];

describe("topology relayout", () => {
  test("is asked for once, then only when the structure changes", async () => {
    const s = scenario();
    expect(await s.relayout()).toBe(1);

    for (let n = 0; n < 1_000; n += 1) s.patch(`agt_${String(n % 300).padStart(5, "0")}`, (agent) => ({ ...agent, status: must(STATES[n % 4], "a state") }));
    expect(await s.relayout()).toBe(1);
    s.patch("agt_00007", (agent) => ({ ...agent, name: "renamed", model: { provider: "codex", model: "x" }, paused: true }));
    expect(await s.relayout()).toBe(1);

    s.add({ ...must(fixtureAgents(2).agt_00001, "agt_00001"), id: "agt_00300", parent: "agt_00000", run: "run_00000" });
    expect(await s.relayout()).toBe(2);
    s.toggle(new Set(["run:aether:fixture/run_00000"]));
    expect(await s.relayout()).toBe(3);
    s.patch("agt_00003", (agent) => ({ ...agent, status: { state: "cancelled" } }));
    expect(await s.relayout()).toBe(3);
    s.toggle(new Set());
    expect(await s.relayout()).toBe(4);
    s.store.updateAether(FIXTURE_CONN, (state) => {
      const agents = { ...state.agents };
      delete agents.agt_00299;
      return { ...state, agents };
    });
    expect(await s.relayout()).toBe(5);
  });

  test("tries a failed layout again for the same structure", async () => {
    let calls = 0;
    const gate = new LayoutGate(async (model) => {
      calls += 1;
      if (calls === 1) throw new Error("worker gone");
      return placeAll(model);
    });
    const model: GraphModel = { nodes: [{ id: "a", label: "a" }], edges: [], groups: [], collapsed: new Set() };
    await expect(gate.layout(model)).rejects.toThrow("worker gone");
    await expect(gate.layout(model)).resolves.toMatchObject({ hash: "test" });
    expect(gate.requests).toBe(2);
  });
});
