// @vitest-environment happy-dom
import type { ConnectionInfo } from "@aether-ui/protocol";
import { act } from "react";
import { afterEach, describe, expect, test } from "vitest";
import { fixtureAgents, fixtureConnection, seedFleet } from "./fixture/agents.ts";
import { harness } from "./fixture/mount.test-util.tsx";
import { must } from "./fixture/must.ts";
import { FleetScreen } from "./FleetScreen.tsx";

afterEach(() => { document.body.innerHTML = ""; });

const archond: ConnectionInfo = { id: "archon:studio", daemon: "archon", state: "ready", features: [], label: "studio" };

async function fleet(count: number, withArchond = false) {
  const h = harness();
  seedFleet(h.app.store, fixtureConnection(), fixtureAgents(count));
  if (withArchond) h.app.store.update((state) => ({ ...state, connections: { ...state.connections, [archond.id]: archond } }));
  const container = await h.mount(<FleetScreen />);
  return { h, container };
}

function pick(container: HTMLElement, label: string, value: string): Promise<void> {
  const field = [...container.querySelectorAll(".fleet-filter")].find((node) => node.querySelector("label")?.textContent === label);
  const select = field?.querySelector("select");
  if (!select) throw new Error(`no ${label} filter`);
  return act(async () => {
    select.value = value;
    select.dispatchEvent(new Event("change", { bubbles: true }));
  });
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
    await pick(container, "State", "=running");
    expect(count(container)).toMatch(/^\d+ of 200 agents$/);
    expect([...container.querySelectorAll(".fleet-state .ui-pill")].every((pill) => pill.textContent === "Running")).toBe(true);
    await pick(container, "State", "");
    await pick(container, "Run", "=aether:fixture/run_00000");
    await pick(container, "Role", "=archon-stage:adaptive/build");
    await pick(container, "Model", "=claude-code/opus");
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
    const roleField = [...container.querySelectorAll(".fleet-filters .fleet-filter")].find(node => node.querySelector("label")?.textContent === "Role");
    const roleOptions = roleField?.querySelectorAll("option") ?? [];
    expect([...roleOptions].filter((option) => option.textContent === "No role (1)").map((option) => option.value)).toEqual(["="]);
    await pick(container, "Role", "=");
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
    for (const select of container.querySelectorAll("select")) {
      const label = select.closest("label")?.textContent ?? container.querySelector(`label[for="${select.id}"]`)?.textContent;
      expect(label).toMatch(/^(Run|Role|State|Model|Archive)/);
    }
    expect(container.querySelector("[role=list]")?.getAttribute("aria-label")).toBe("Agents");
    expect(container.querySelector("[role=group]")?.getAttribute("aria-label")).toBe("Filter agents");
  });

  test("moves archived agents out of the default fleet and back on restore", async () => {
    const { h, container } = await fleet(2);
    h.app.store.updateAether("aether:fixture", state => ({ ...state, agents: { ...state.agents, agt_00000: { ...must(state.agents.agt_00000, "agent"), archived: true } } }));
    await h.frame();
    expect(count(container)).toBe("1 of 1 agents");
    const archive = container.querySelector<HTMLSelectElement>('.fleet-bar > label select');
    if (!archive) throw new Error("Archive filter missing");
    await act(async () => { archive.value = "archived"; archive.dispatchEvent(new Event("change", { bubbles: true })); });
    expect(container.querySelector('[data-agent="agt_00000"]')).not.toBeNull();
    h.app.store.updateAether("aether:fixture", state => ({ ...state, agents: { ...state.agents, agt_00000: { ...must(state.agents.agt_00000, "agent"), archived: false } } }));
    await h.frame();
    expect(container.textContent).toContain("No archived agents");
  });
});
