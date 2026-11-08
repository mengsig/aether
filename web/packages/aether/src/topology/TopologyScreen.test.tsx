// @vitest-environment happy-dom
// The mounted screen asks for a layout only on structural events, whatever the store does between them.

import type { GraphModel, Layout } from "@aether-ui/protocol";
import { act } from "react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { fixtureAgents, fixtureConnection, FIXTURE_CONN, seedFleet } from "../fleet/fixture/agents.ts";
import { harness } from "../fleet/fixture/mount.test-util.tsx";
import { must } from "../fleet/fixture/must.ts";
import { TopologyScreen } from "./TopologyScreen.tsx";

function stubCanvas(): void {
  const ctx = {
    fillStyle: "", strokeStyle: "", lineWidth: 1, font: "", textBaseline: "alphabetic",
    setTransform: () => undefined, clearRect: () => undefined, strokeRect: () => undefined, beginPath: () => undefined,
    moveTo: () => undefined, lineTo: () => undefined, bezierCurveTo: () => undefined, stroke: () => undefined,
    fillRect: () => undefined, fillText: () => undefined, measureText: (text: string) => ({ width: text.length * 7 }),
  };
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(ctx as unknown as CanvasRenderingContext2D);
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(800);
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(400);
  vi.stubGlobal("ResizeObserver", class {
    readonly callback: () => void;
    constructor(callback: () => void) { this.callback = callback; }
    observe() { this.callback(); }
    disconnect() { return undefined; }
  });
}

beforeEach(stubCanvas);
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

function countingLayout() {
  const calls: GraphModel[] = [];
  const layout = async (model: GraphModel): Promise<Layout> => {
    calls.push(model);
    return { hash: String(calls.length), positions: new Map(model.nodes.map((node, i) => [node.id, { x: i * 200, y: 0 }])) };
  };
  return { calls, layout };
}

async function topology(count: number) {
  const h = harness();
  seedFleet(h.app.store, fixtureConnection(), fixtureAgents(count));
  const counting = countingLayout();
  const container = await h.mount(<TopologyScreen layout={counting.layout} />);
  await h.frame();
  return { h, container, calls: counting.calls };
}

const button = (container: HTMLElement, text: string) => {
  const found = [...container.querySelectorAll("button")].find((node) => node.textContent === text);
  if (!found) throw new Error(`no ${text} button`);
  return found;
};

describe("the topology screen", () => {
  test("lays out again for an added agent or a collapse, never for status changes", async () => {
    const { h, container, calls } = await topology(40);
    expect(calls).toHaveLength(1);
    for (let n = 0; n < 30; n += 1) {
      h.app.store.updateAether(FIXTURE_CONN, (state) => ({
        ...state, agents: { ...state.agents, agt_00001: { ...must(state.agents.agt_00001, "agt_00001"), status: n % 2 ? { state: "running" } : { state: "completed" } } },
      }));
      await h.frame();
    }
    expect(calls).toHaveLength(1);
    h.app.store.updateAether(FIXTURE_CONN, (state) => ({
      ...state, agents: { ...state.agents, agt_00040: { ...must(state.agents.agt_00002, "agt_00002"), id: "agt_00040", name: "late" } },
    }));
    await h.frame();
    expect(calls).toHaveLength(2);
    await act(async () => button(container, "Collapse run").click());
    expect(calls).toHaveLength(3);
    expect(calls[2]?.collapsed.size).toBe(calls[2]?.groups.length);
  });

  test("collapses a small selected run on its first group click in a large fleet", async () => {
    const { container, calls } = await topology(410);
    const group = container.querySelector<HTMLButtonElement>('button[aria-expanded="true"]');
    if (!group) throw new Error("expanded group missing");
    expect(calls.at(-1)?.collapsed.size).toBe(0);
    await act(async () => group.click());
    expect(calls.at(-1)?.collapsed.size).toBe(1);
  });

  test("labels Archon-run agents by role in the graph's node list without archond", async () => {
    const { container } = await topology(6);
    const items = [...container.querySelectorAll("li")].map((node) => node.textContent ?? "");
    expect(items.some((text) => text.includes("qi-00000 · Archon run · adaptive"))).toBe(true);
    expect(items.some((text) => text.includes("plan-1 · Archon plan · adaptive"))).toBe(true);
  });

  test("prioritizes active runs and lets a user search and select a different run", async () => {
    const h = harness();
    const agents = fixtureAgents(30);
    for (const agent of Object.values(agents)) {
      agents[agent.id] = { ...agent, status: { state: "completed" }, updated_at: "2026-11-05T08:00:00Z" };
    }
    agents.agt_00003 = { ...must(agents.agt_00003, "agt_00003"), status: { state: "ready" }, updated_at: "2026-11-04T08:00:00Z" };
    seedFleet(h.app.store, fixtureConnection(), agents);
    const counting = countingLayout();
    const container = await h.mount(<TopologyScreen layout={counting.layout} />);
    await h.frame();
    expect(counting.calls[0]?.groups[0]?.id).toContain("run_00001");
    expect(container.querySelector('[aria-pressed="true"]')?.textContent).toContain("Active or pending");
    const search = container.querySelector<HTMLInputElement>('input[type="search"]');
    if (!search) throw new Error("missing run search");
    await act(async () => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(search, "qi-00000"); search.dispatchEvent(new Event("input", { bubbles: true })); });
    const options = container.querySelectorAll<HTMLButtonElement>(".topology-run");
    expect(options).toHaveLength(1);
    await act(async () => options[0]?.click());
    expect(counting.calls.at(-1)?.groups[0]?.id).toContain("run_00000");
    expect(container.querySelector('[aria-pressed="true"]')?.textContent).toContain("qi-00000");
    expect(container.querySelector('[aria-label="Graph zoom"]')?.textContent).toContain("Zoom in");
  });

  test("shows a failed layout with a retry that asks again", async () => {
    const h = harness();
    seedFleet(h.app.store, fixtureConnection(), fixtureAgents(3));
    let calls = 0;
    const layout = async (model: GraphModel): Promise<Layout> => {
      calls += 1;
      if (calls === 1) throw new Error("the layout worker stopped");
      return { hash: "ok", positions: new Map(model.nodes.map((node) => [node.id, { x: 0, y: 0 }])) };
    };
    const container = await h.mount(<TopologyScreen layout={layout} />);
    await h.frame();
    expect(container.querySelector("[role=alert]")?.textContent).toContain("the layout worker stopped");
    await act(async () => button(container, "Try again").click());
    expect(calls).toBe(2);
    expect(container.querySelector("[role=alert]")).toBeNull();
  });

  test("names the graph and lists its runs for screen readers", async () => {
    const { container } = await topology(40);
    expect(container.querySelector("canvas")?.getAttribute("aria-label")).toBe("Delegation topology");
    expect([...container.querySelectorAll("li button")].filter((node) => node.textContent?.startsWith("Group Run ")).length).toBe(1);
  });
  test("offers state context, agent details and a guided empty state", async () => {
    const { h, container } = await topology(3);
    expect(container.querySelector("[data-testid=graph-legend]")?.textContent).toContain("Completed");
    const canvas = container.querySelector("canvas");
    await act(async () => { canvas?.dispatchEvent(new KeyboardEvent("keydown", { key: "Home", bubbles: true })); });
    const details = container.querySelector("[data-testid=graph-details]");
    expect(details?.textContent).toContain("Cost:");
    expect(details?.querySelector("a")?.getAttribute("href")).toContain("#/agents/");
    seedFleet(h.app.store, fixtureConnection(), fixtureAgents(0));
    await h.frame();
    expect(container.textContent).toContain("Start an agent from Activity.");
    expect(container.querySelector("a[data-variant=primary]")?.getAttribute("href")).toBe("#/activity");
  });

});
