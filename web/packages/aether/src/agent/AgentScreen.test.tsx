// @vitest-environment happy-dom
import type { aether } from "@aether-ui/protocol";
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

const task = (id: string, objective: string, state: aether.TaskStatusKind, created_at: string): aether.Task => ({
  id, objective, owner: FAUX_AGENT, run: "run_01JFAUX0000000000000000001", creator: { actor: "user" },
  status: { state } as aether.TaskStatus, created_at, updated_at: created_at,
});

async function screen(agent = fauxAgent(), id = FAUX_AGENT, tasks: readonly aether.Task[] = []) {
  const h = await connected({ instance: "faux-1", agents: [fauxAgent(FAUX_PARENT), agent], tasks });
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

  test("names the task the agent holds first among its facts, and the newest open one when it holds none", async () => {
    const tasks = [
      task("tsk_done", "Write the lexer.", "completed", "2026-11-04T08:00:00Z"),
      task("tsk_old", "Write the parser.", "assigned", "2026-11-04T08:30:00Z"),
      task("tsk_new", "Write the release note.", "active", "2026-11-04T09:00:00Z"),
    ];
    const held = await screen(fauxAgent(FAUX_AGENT, { current_task: "tsk_old" }), FAUX_AGENT, tasks);
    expect(held.container.querySelector(".agent-fact")?.getAttribute("data-testid")).toBe("agent-fact-task");
    expect(fact(held.container, "Task")).toBe("Write the parser.assigned");
    held.h.unmount();
    const idle = await screen(fauxAgent(), FAUX_AGENT, tasks);
    expect(fact(idle.container, "Task")).toBe("Write the release note.active");
    idle.h.unmount();
    const { container } = await screen(fauxAgent(), FAUX_AGENT, tasks.slice(0, 1));
    expect(fact(container, "Task")).toBeUndefined();
    expect(container.querySelector(".agent-fact")?.getAttribute("data-testid")).toBe("agent-fact-model");
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
