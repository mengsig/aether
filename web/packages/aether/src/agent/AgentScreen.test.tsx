// @vitest-environment happy-dom
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
  h.daemon.seed(FAUX_AGENT, [{ message: text("user", "Your task:\nBuild it.") }, { message: text("assistant", "On it."), invocation: "inv_1" }]);
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
    const task = container.querySelector(".conv-card[data-kind=\"user_task\"]");
    expect(task?.querySelector(".conv-card-title")?.textContent, "a delegated agent's first message is its task").toBe("Task from planner");
    expect(task?.querySelector("a")?.getAttribute("href")).toContain(encodeURIComponent(FAUX_PARENT));
    expect([...container.querySelectorAll(".conv-msg-body")].map((node) => node.querySelector(".conv-card-text")?.textContent ?? node.textContent)).toEqual(["Build it.", "On it."]);
    expect(container.querySelector('textarea[aria-label="Message builder"]'), "the chat has its composer").not.toBeNull();
  });

  test("updates from the store when a refetched row replaces the agent", async () => {
    const { h, container } = await screen();
    h.app.store.updateAether(h.conn, (state) => ({ ...state, agents: { ...state.agents, [FAUX_AGENT]: fauxAgent(FAUX_AGENT, { status: { state: "failed", error: "the engine exited" }, updated_at: "2026-11-04T10:00:00Z" }) } }));
    await h.settle();
    expect(container.querySelector(".agent-head .ui-pill")?.textContent).toBe("Failed");
    expect(container.querySelector(".agent-detail")?.textContent).toBe("the engine exited");
  });

  test("shows pending approvals for this agent and clears the state after a decision", async () => {
    const { h, container } = await screen();
    h.app.store.updateAether(h.conn, state => ({ ...state, attention: { last_seq: 1, questions: [], agents: [], approvals: [{ id: "apr_1", agent_id: FAUX_AGENT, call_id: "call_1", tool: "read", description: "Read file", argument: "", side_effect: "read_only", status: { state: "pending" }, requested_at: "2026-11-04" }] } }));
    await h.settle();
    expect(container.querySelector(".agent-head .ui-pill")?.textContent).toBe("Waiting for your approval");
    expect(container.querySelector('a[href="#/inbox"]')?.textContent).toContain("Review approval");
    h.app.store.updateAether(h.conn, state => ({ ...state, attention: { last_seq: 2, questions: [], agents: [], approvals: [] } }));
    await h.settle();
    expect(container.querySelector(".agent-head .ui-pill")?.textContent).toBe("Running");
  });

  test("reads an agent absent from the fleet and opens its conversation", async () => {
    const h = current = await connected({ instance: "faux-1", agents: [] });
    h.daemon.agents.set(FAUX_AGENT, fauxAgent());
    h.daemon.seed(FAUX_AGENT, [{ message: text("user", "Your task:\nEarlier work") }]);
    const container = await h.mount(<AgentRoute params={{ conn: h.conn, agent: FAUX_AGENT }} />);
    await h.settle();
    await h.settle();
    expect(h.daemon.commands("get_agent")).toEqual([expect.objectContaining({ id: FAUX_AGENT })]);
    expect(container.querySelector(".agent-name")?.textContent).toBe("builder");
    expect(container.textContent).toContain("Earlier work");
    expect(container.querySelector('.conv-card[data-kind="user_task"]')).not.toBeNull();
  });

  test("reports an unavailable agent and retries the lookup", async () => {
    const { container } = await screen(fauxAgent(), "agt_missing");
    expect(container.textContent).toContain("Agent unavailable");
    expect(container.textContent).toContain("may no longer be available");
    expect(container.textContent).toContain("Agent not found");
    const h = current;
    expect(h).not.toBeNull();
    h?.daemon.agents.set("agt_missing", fauxAgent("agt_missing"));
    const retry = [...container.querySelectorAll("button")].find((button) => button.textContent === "Try again");
    expect(retry).toBeDefined();
    retry?.click();
    await h?.settle();
    await h?.settle();
    expect(container.querySelector(".agent-name")).not.toBeNull();
  });

  test("says so for an unknown connection", async () => {
    await screen();
    current?.unmount();
    const h = await connected({ instance: "faux-2", agents: [] });
    current = h;
    const other = await h.mount(<AgentRoute params={{ conn: "aether:gone", agent: FAUX_AGENT }} />);
    expect(other.textContent).toContain("Not connected");
  });
});
