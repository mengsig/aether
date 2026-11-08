// @vitest-environment happy-dom
import { drafts } from "@aether-ui/core";
import type { ConnectionInfo } from "@aether-ui/protocol";
import { act } from "react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { fixtureAgents, fixtureConnection, seedFleet } from "./fixture/agents.ts";
import { harness } from "./fixture/mount.test-util.tsx";
import { must } from "./fixture/must.ts";
import { FleetScreen } from "./FleetScreen.tsx";

beforeEach(() => { for (const key of drafts.keys()) drafts.clear(key); });
afterEach(() => { document.body.innerHTML = ""; });

const archond: ConnectionInfo = { id: "archon:studio", daemon: "archon", state: "ready", features: [], label: "studio" };

async function fleet(count: number, withArchond = false) {
  const h = harness();
  seedFleet(h.app.store, fixtureConnection(), fixtureAgents(count));
  if (withArchond) h.app.store.update((state) => ({ ...state, connections: { ...state.connections, [archond.id]: archond } }));
  const container = await h.mount(<FleetScreen />);
  return { h, container };
}

async function pick(container: HTMLElement, label: string, value: string): Promise<void> {
  const field = [...container.querySelectorAll(".ui-picker")].find(node => node.querySelector("label")?.textContent === label);
  const trigger = field?.querySelector<HTMLButtonElement>('[role="combobox"]');
  if (!field || !trigger) throw new Error(`no ${label} filter`);
  await act(async () => trigger.click());
  const option = [...field.querySelectorAll<HTMLElement>('[role="option"]')].find(o => {
    const text = o.querySelector(".ui-picker-option-label")?.textContent;
    return value === "" ? text === "All" : text === value || text?.startsWith(`${value} (`);
  });
  if (!option) throw new Error(`no ${value} option`);
  await act(async () => option.click());
}

const count = (container: HTMLElement) => container.querySelector(".fleet-count")?.textContent;

describe("the fleet list", () => {
  test("keeps 5,000 agents to a screenful of DOM rows", async () => {
    const { container } = await fleet(5_000);
    const rows = container.querySelectorAll("[role=listitem]");
    expect(rows.length).toBeGreaterThan(5);
    expect(rows.length).toBeLessThan(60);
    expect(rows[0]?.getAttribute("aria-setsize")).toBe("5000");
    expect(count(container)).toBe("5,000 of 5,000 agents");
  });

  test("filters by run, role, state and model, and clears back to all", async () => {
    const { container } = await fleet(200);
    await pick(container, "State", "running");
    expect(count(container)).toMatch(/^\d+ of 200 agents$/);
    expect([...container.querySelectorAll(".fleet-state .ui-pill")].every((pill) => pill.textContent === "Running")).toBe(true);
    await pick(container, "State", "");
    await pick(container, "Run", "qi-00000 (Archon run · adaptive)");
    await pick(container, "Role", "Archon build · adaptive");
    await pick(container, "Model", "claude-code/opus");
    expect(count(container)).toBe("0 of 200 agents");
    const clear = [...container.querySelectorAll("button")].find((button) => button.textContent === "Clear filters");
    await act(async () => clear?.click());
    expect(count(container)).toBe("200 of 200 agents");
  });

  test("offers No role apart from All, and clears back to All", async () => {
    const h = harness();
    const agents = fixtureAgents(5);
    agents.agt_00003 = { ...must(agents.agt_00003, "agt_00003"), role: "" };
    seedFleet(h.app.store, fixtureConnection(), agents);
    const container = await h.mount(<FleetScreen />);
    await pick(container, "Role", "No role");
    expect(count(container)).toBe("1 of 5 agents");
    await pick(container, "Role", "");
    expect(count(container)).toBe("5 of 5 agents");
  });

  test("labels Archon-run agents by role with or without archond", async () => {
    for (const withArchond of [false, true]) {
      const { container } = await fleet(12, withArchond);
      const roles = [...container.querySelectorAll(".fleet-role")].map((node) => node.textContent);
      expect(roles.slice(0, 3)).toEqual(["Archon run · adaptive", "Archon plan · adaptive", "Archon build · adaptive"]);
      expect(container.querySelector(".fleet-role")?.getAttribute("title")).toBe("archon-run:adaptive");
      document.body.innerHTML = "";
    }
  });

  test("updates a row in place when an agent's state changes", async () => {
    const { h, container } = await fleet(10);
    h.app.store.updateAether("aether:fixture", (state) => ({
      ...state, agents: { ...state.agents, agt_00000: { ...must(state.agents.agt_00000, "agt_00000"), status: { state: "completed" } } },
    }));
    await h.frame();
    expect(container.querySelector("[data-agent=agt_00000] .ui-pill")?.textContent).toBe("Completed");
  });

  test("says so when no Aether daemon is connected", async () => {
    const container = await harness().mount(<FleetScreen />);
    expect(container.textContent).toContain("No Aether daemon connected");
  });

  test("names every control and the list, for assistive technology", async () => {
    const { container } = await fleet(300);
    const controls = container.querySelectorAll('[role="combobox"]');
    expect(controls).toHaveLength(5);
    for (const control of controls) {
      expect(control.closest(".ui-picker")?.querySelector("label")?.textContent).toMatch(/^(Run|Role|State|Model|Archive)$/);
    }
    expect(container.querySelector("[role=list]")?.getAttribute("aria-label")).toBe("Agents");
    expect(container.querySelector("[role=group]")?.getAttribute("aria-label")).toBe("Filter agents");
  });

  test("loads archived agents again after reconnect while the archived view is open", async () => {
    const h = harness();
    const info = fixtureConnection();
    const archived = { ...must(fixtureAgents(1).agt_00000, "agent"), archived: true };
    seedFleet(h.app.store, info, {});
    const send = vi.spyOn(h.app, "send").mockResolvedValue({ reply: "agents_page", data: { agents: [archived] } });
    const container = await h.mount(<FleetScreen />);
    await pick(container, "Archive", "Archived");
    await h.frame();
    expect(container.querySelector('[data-agent="agt_00000"]')).not.toBeNull();
    expect(send).toHaveBeenCalledWith(info.id, expect.objectContaining({ query: { include_archived: true } }));
    h.app.store.update((state) => ({ ...state, connections: { ...state.connections, [info.id]: { ...info, state: "backoff" } } }));
    h.app.store.updateAether(info.id, (state) => ({ ...state, agents: {} }));
    await h.frame();
    h.app.store.update((state) => ({ ...state, connections: { ...state.connections, [info.id]: info } }));
    await h.frame();
    await h.frame();
    expect(send.mock.calls.length).toBeGreaterThanOrEqual(2);
    expect(container.querySelector('[data-agent="agt_00000"]')).not.toBeNull();
    expect(send).toHaveBeenCalledTimes(2);
  });

  test("shows loading and retries a failed archive request", async () => {
    const h = harness();
    seedFleet(h.app.store, fixtureConnection(), {});
    let rejectRequest: (error: Error) => void = () => { throw new Error("request not started"); };
    const send = vi.spyOn(h.app, "send").mockImplementationOnce(() => new Promise((_resolve, reject) => { rejectRequest = reject; }))
      .mockResolvedValueOnce({ reply: "agents_page", data: { agents: [] } });
    const container = await h.mount(<FleetScreen />);
    const select = container.querySelector<HTMLSelectElement>(".fleet-bar > label select");
    if (!select) throw new Error("Archive filter missing");
    await act(async () => { select.value = "archived"; select.dispatchEvent(new Event("change", { bubbles: true })); });
    expect(container.textContent).toContain("Loading archived agents");
    expect(container.textContent).not.toContain("No archived agents");
    await act(async () => rejectRequest(new Error("offline")));
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("offline");
    expect(container.textContent).not.toContain("No archived agents");
    const retry = must([...container.querySelectorAll("button")].find(button => button.textContent === "Retry"), "retry button");
    await act(async () => retry.click());
    expect(send).toHaveBeenCalledTimes(2);
    expect(container.textContent).toContain("No archived agents");
  });

  test("live agent updates do not restart an in-flight archive load", async () => {
    const h = harness();
    const info = fixtureConnection();
    seedFleet(h.app.store, info, fixtureAgents(1));
    const archived = { ...must(fixtureAgents(2).agt_00001, "agent"), archived: true };
    let resolveRequest: (reply: { reply: string; data: { agents: typeof archived[] } }) => void = () => { throw new Error("request not started"); };
    const send = vi.spyOn(h.app, "send").mockImplementation(() => new Promise(resolve => { resolveRequest = resolve; }));
    const container = await h.mount(<FleetScreen />);
    const select = container.querySelector<HTMLSelectElement>(".fleet-bar > label select");
    if (!select) throw new Error("Archive filter missing");
    await act(async () => { select.value = "archived"; select.dispatchEvent(new Event("change", { bubbles: true })); });
    h.app.store.updateAether(info.id, state => ({ ...state, agents: { ...state.agents, agt_00000: { ...must(state.agents.agt_00000, "agent"), name: "updated" } } }));
    await h.frame();
    expect(send).toHaveBeenCalledTimes(1);
    await act(async () => resolveRequest({ reply: "agents_page", data: { agents: [archived] } }));
    await h.frame();
    expect(container.querySelector('[data-agent="agt_00001"]')).not.toBeNull();
    expect(send).toHaveBeenCalledTimes(1);
  });

  test("ignores an archive response after leaving the view", async () => {
    const h = harness();
    const info = fixtureConnection();
    seedFleet(h.app.store, info, {});
    const archived = { ...must(fixtureAgents(1).agt_00000, "agent"), archived: true };
    let resolveRequest: (reply: { reply: string; data: { agents: typeof archived[] } }) => void = () => { throw new Error("request not started"); };
    vi.spyOn(h.app, "send").mockImplementation(() => new Promise(resolve => { resolveRequest = resolve; }));
    const container = await h.mount(<FleetScreen />);
    const select = container.querySelector<HTMLSelectElement>(".fleet-bar > label select");
    if (!select) throw new Error("Archive filter missing");
    await act(async () => { select.value = "archived"; select.dispatchEvent(new Event("change", { bubbles: true })); });
    await act(async () => { select.value = "active"; select.dispatchEvent(new Event("change", { bubbles: true })); });
    await act(async () => resolveRequest({ reply: "agents_page", data: { agents: [archived] } }));
    expect(h.app.store.get().aether[info.id]?.agents[archived.id]).toBeUndefined();
    expect(container.textContent).not.toContain("Loading archived agents");
  });

  test("moves archived agents out of the default fleet and back on restore", async () => {
    const { h, container } = await fleet(2);
    vi.spyOn(h.app, "send").mockResolvedValue({ reply: "agents_page", data: { agents: [] } });
    h.app.store.updateAether("aether:fixture", state => ({ ...state, agents: { ...state.agents, agt_00000: { ...must(state.agents.agt_00000, "agent"), archived: true } } }));
    await h.frame();
    expect(count(container)).toBe("1 of 1 agents");
    await pick(container, "Archive", "Archived");
    expect(container.querySelector('[data-agent="agt_00000"]')).not.toBeNull();
    h.app.store.updateAether("aether:fixture", state => ({ ...state, agents: { ...state.agents, agt_00000: { ...must(state.agents.agt_00000, "agent"), archived: false } } }));
    await h.frame();
    expect(container.textContent).toContain("No archived agents");
  });
});
