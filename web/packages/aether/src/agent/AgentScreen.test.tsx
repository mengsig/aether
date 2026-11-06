// @vitest-environment happy-dom
import { act } from "react";
import { afterEach, describe, expect, test } from "vitest";
import { connected, type Harness } from "../conversation/fixture/harness.test-util.tsx";
import { FAUX_AGENT, FAUX_PARENT, fauxAgent, text } from "../conversation/fixture/transcript.ts";
import AgentRoute from "./AgentScreen.tsx";
import routes from "./routes.ts";

let current: Harness | null = null;
afterEach(() => {
  current?.unmount();
  current = null;
});

async function screen(agent = fauxAgent(), id = FAUX_AGENT) {
  const h = await connected({ instance: "faux-1", agents: [fauxAgent(FAUX_PARENT), agent] });
  current = h;
  h.daemon.seed(FAUX_AGENT, [{ message: text("user", "Build it.") }, { message: text("assistant", "On it."), invocation: "inv_1" }]);
  const container = await h.mount(<AgentRoute params={{ conn: h.conn, agent: id }} />);
  await h.settle();
  return { h, container };
}

const fact = (container: HTMLElement, label: string) =>
  [...container.querySelectorAll(".agent-fact")].find((node) => node.querySelector("dt")?.textContent === label)?.querySelector("dd")?.textContent;

describe("the agent screen", () => {
  test("registers #/agents/:conn/:agent for B2", () => {
    expect(routes.map((route) => [route.path, route.owner])).toEqual([["#/agents/:conn/:agent", "B2"]]);
  });

  test("shows the agent's state, role, parent, model, cost and its conversation", async () => {
    const { container } = await screen();
    expect(container.querySelector(".agent-name")?.textContent).toBe("builder");
    expect(container.querySelector(".agent-head .ui-pill")?.textContent).toBe("Running");
    expect(container.querySelector(".agent-sub")?.textContent).toContain("Archon build · adaptive");
    expect(container.querySelector(".agent-sub a")?.getAttribute("href")).toBe(`#/agents/aether%3Afaux-1/${FAUX_PARENT}`);
    expect(container.querySelector(".agent-sub a")?.textContent).toBe("planner");
    expect(fact(container, "Model")).toBe("claude-code/opuseffort high");
    expect(fact(container, "Cost")).toBe("$7.43");
    expect(fact(container, "Tokens")).toBe("1.2M in · 88k out");
    expect(fact(container, "Turns")).toBe("42");
    expect(fact(container, "Budget")).toBe("$25.00 · 4.0M tokens");
    expect([...container.querySelectorAll(".conv-msg-body")].map((node) => node.textContent)).toEqual(["Build it.", "On it."]);
  });

  test("shows model, cost and turns in the header and opens the details on demand", async () => {
    const { container } = await screen();
    expect([...container.querySelectorAll(".agent-headline > span")].map((node) => node.textContent)).toEqual(["claude-code/opus", "$7.43", "42 turns"]);
    const toggle = container.querySelector<HTMLButtonElement>(".agent-details-toggle");
    const aside = container.querySelector(".agent-aside");
    expect(toggle?.getAttribute("aria-controls")).toBe(aside?.id);
    expect(toggle?.getAttribute("aria-expanded")).toBe("false");
    expect(aside?.getAttribute("data-open")).toBe("false");
    await act(async () => toggle?.click());
    expect(toggle?.getAttribute("aria-expanded")).toBe("true");
    expect(aside?.getAttribute("data-open")).toBe("true");
  });

  test("updates from the store when a refetched row replaces the agent", async () => {
    const { h, container } = await screen();
    h.app.store.updateAether(h.conn, (state) => ({ ...state, agents: { ...state.agents, [FAUX_AGENT]: fauxAgent(FAUX_AGENT, { status: { state: "failed", error: "the engine exited" }, updated_at: "2026-11-04T10:00:00Z" }) } }));
    await h.settle();
    expect(container.querySelector(".agent-head .ui-pill")?.textContent).toBe("Failed");
    expect(container.querySelector(".agent-detail")?.textContent).toBe("the engine exited");
  });

  test("says so for an unknown agent or connection", async () => {
    const { container } = await screen(fauxAgent(), "agt_missing");
    expect(container.textContent).toContain("No such agent");
    current?.unmount();
    const h = await connected({ instance: "faux-2", agents: [] });
    current = h;
    const other = await h.mount(<AgentRoute params={{ conn: "aether:gone", agent: FAUX_AGENT }} />);
    expect(other.textContent).toContain("Not connected");
  });
});
