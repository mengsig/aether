// The fleet at 5,000 agents: mount, filter, scroll and one agent's row changing.

import type { Store } from "@aether-ui/core";
import type { aether, ConnectionInfo } from "@aether-ui/protocol";
import { act, createElement, type ComponentProps } from "react";
import { afterAll, describe, expect, test, vi } from "vitest";
import { FleetScreen } from "../../packages/aether/src/fleet/FleetScreen.tsx";
import { agentId, FLEET_SIZE, fleetAgents, fleetRoots } from "../fixtures/fleet.ts";
import { heapUsed, mount, perfApp, Results, timed, type Mounted } from "../measure.tsx";
import { renders, resetRenders } from "../renders.ts";

// Every fleet row draws one StatusPill, so pill renders count row renders.
vi.mock("@aether-ui/ui", async (importOriginal) => {
  const ui = await importOriginal<typeof import("@aether-ui/ui")>();
  const counter = await import("../renders.ts");
  const StatusPill = (props: ComponentProps<typeof ui.StatusPill>) => {
    counter.renders.pill += 1;
    return createElement(ui.StatusPill, props);
  };
  return { ...ui, StatusPill };
});

const SEED = 5_000;
const CONN = "aether:perf";
const INFO: ConnectionInfo = { id: CONN, daemon: "aether", state: "ready", protocol: 26, features: ["agents_paging"], standing: "operator", label: "perf" };
const ROW_HEIGHT = 44;
const results = new Results("fleet");
afterAll(() => results.write());

function seed(store: Store, agents: Record<string, aether.Agent>): void {
  store.update((state) => ({ ...state, connections: { ...state.connections, [CONN]: INFO } }));
  store.updateAether(CONN, (state) => ({ ...state, agents, roots: fleetRoots(agents) }));
}

async function mountFleet(agents: Record<string, aether.Agent>): Promise<Mounted> {
  const { app, frames } = perfApp();
  seed(app.store, agents);
  return mount(app, frames, <FleetScreen />);
}

function stateSelect(container: HTMLElement): HTMLSelectElement {
  const field = [...container.querySelectorAll(".fleet-filter")].find((node) => node.querySelector("label")?.textContent === "State");
  const select = field?.querySelector("select");
  if (!select) throw new Error("the fleet has no State filter");
  return select;
}

/** The store as core leaves it after `get_agent` answers for a changed row (aether-sync withAgent). */
function changeAgent(store: Store, id: string, status: aether.AgentStatus, at: string): void {
  store.updateAether(CONN, (state) => {
    const held = state.agents[id];
    if (!held) throw new Error(`agent ${id} is not in the fleet`);
    return { ...state, agents: { ...state.agents, [id]: { ...held, status, updated_at: at } } };
  });
}

describe("the fleet at 5,000 agents", () => {
  const agents = fleetAgents(SEED);

  test("mounts a screenful of rows", async () => {
    expect(Object.keys(agents)).toHaveLength(FLEET_SIZE);
    const mountTime = await timed(async () => { (await mountFleet(agents)).unmount(); });
    resetRenders();
    const before = heapUsed();
    const fleet = await mountFleet(agents);
    const retained = heapUsed() - before;
    expect(fleet.container.querySelector(".fleet-count")?.textContent).toBe("5,000 of 5,000 agents");
    expect(fleet.container.querySelector("[role=listitem]")?.getAttribute("aria-setsize")).toBe(String(FLEET_SIZE));
    results.time("fleet.mount.ms", mountTime);
    results.count("fleet.mount.commits", fleet.counters.commits);
    results.count("fleet.mount.dom_elements", fleet.container.querySelectorAll("*").length);
    results.count("fleet.mount.row_renders", renders.pill);
    results.amount("fleet.mount.heap_bytes", retained);
    fleet.unmount();
  });

  test("filters by state and back", async () => {
    const fleet = await mountFleet(agents);
    const select = stateSelect(fleet.container);
    const pick = (value: string) => fleet.step(() => {
      select.value = value;
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await fleet.mutations();
    resetRenders();
    await pick("=running");
    expect(fleet.container.querySelector(".fleet-count")?.textContent).toMatch(/^[\d,]+ of 5,000 agents$/);
    results.count("fleet.filter.row_renders", renders.pill);
    results.count("fleet.filter.dom_mutations", await fleet.mutations());
    results.time("fleet.filter.ms", await timed(() => pick("=running"), () => pick("")));
    fleet.unmount();
  });

  test("scrolls a page at a time", async () => {
    const fleet = await mountFleet(agents);
    const list = fleet.container.querySelector<HTMLElement>(".ui-vlist");
    if (!list) throw new Error("the fleet has no virtual list");
    let page = 0;
    const scroll = () => fleet.step(() => {
      page += 1;
      list.scrollTop = page * 12 * ROW_HEIGHT;
      list.dispatchEvent(new Event("scroll"));
    });
    await fleet.mutations();
    resetRenders();
    await scroll();
    expect(fleet.container.querySelector("[role=listitem]")?.getAttribute("aria-posinset")).not.toBe("1");
    results.count("fleet.scroll.row_renders", renders.pill);
    results.count("fleet.scroll.dom_mutations", await fleet.mutations());
    results.time("fleet.scroll.ms", await timed(scroll));
    expect(page * 12 * ROW_HEIGHT).toBeLessThan(FLEET_SIZE * ROW_HEIGHT);
    fleet.unmount();
  });

  test("applies one changed agent row", async () => {
    const fleet = await mountFleet(agents);
    const store = fleet.app.store;
    let n = 0;
    const change = () => fleet.step(() => {
      n += 1;
      changeAgent(store, agentId(3), n % 2 === 0 ? { state: "running" } : { state: "completed" }, `2026-11-05T00:00:${String(n % 60).padStart(2, "0")}Z`);
    });
    await act(async () => undefined);
    await fleet.mutations();
    resetRenders();
    const commits = fleet.counters.commits;
    await change();
    results.count("fleet.delta.commits", fleet.counters.commits - commits);
    results.count("fleet.delta.row_renders", renders.pill);
    results.count("fleet.delta.dom_mutations", await fleet.mutations());
    results.time("fleet.delta.ms", await timed(change));
    fleet.unmount();
  });
});
