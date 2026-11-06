// @vitest-environment happy-dom
import { drafts } from "@aether-ui/core";
import { beforeEach, afterEach, expect, test, vi } from "vitest";
import { act } from "react";
import type { aether, DesktopBridge } from "@aether-ui/protocol";
import { connected, click, buttonNamed, pick, type, type Harness } from "../controls/fixture/harness.test-util.tsx";
import { fauxAgent } from "../conversation/fixture/transcript.ts";
import ActivityScreen from "./ActivityScreen.tsx";
let h: Harness;
afterEach(() => h?.unmount());
const agent = fauxAgent();
const event = (seq: number): aether.Event => ({ id: `evt_${seq}`, seq, at: "2026-11-04", actor: { actor: "system" }, agent_id: agent.id, kind: { type: "agent.ready" } });
const script = {
  auth_status: () => ({ tag: "auth", data: [] }),
  list_models: () => ({ tag: "models", data: [{ name: "Astra", provider_name: "Codex", capabilities: {}, configured: true, is_default: true, model: { provider: "codex", model: "astra" } }] }),
  list_workspaces: () => ({ tag: "workspaces", data: [{ path: "/repo", exists: true }] }),
  validate_workspace: () => ({ tag: "workspace_validation", data: { valid: true, path: "/repo" } }),
  get_git_summary: () => ({ tag: "git_summary", data: { repository: true, branch: "main", dirty: false } }),
  complete_path: () => ({ tag: "path_completions", data: { entries: [{ path: "/repo", is_dir: true }], truncated: false } }),
  create_agent: () => ({ tag: "agent", data: agent }),
  create_task: () => ({ tag: "task", data: { id: "task_1" } }),
};

test("shows live events, backward store pages, filters, and agent links", async () => {
  h = await connected({ agents: [agent], events: [event(20)], script: { get_events: () => ({ tag: "events", data: [event(10)] }) } });
  const root = await h.mount(<ActivityScreen />);
  await act(async () => { h.daemon.emit(agent.id, { type: "invocation.completed" }); }); await h.settle();
  expect(root.textContent).toContain("invocation.completed");
  expect(h.daemon.commands("get_events")[0]).toMatchObject({ after: 0, limit: 100, query: { before: Number.MAX_SAFE_INTEGER } });
  expect(h.app.store.get().reads[h.conn]?.[`activity:before:${Number.MAX_SAFE_INTEGER}`]?.value).toEqual([event(10)]);
  await click(h, root, "Show older"); await h.settle();
  expect(h.daemon.commands("get_events").at(-1)).toMatchObject({ after: 0, limit: 100, query: { before: 10 } });
  expect(root.querySelectorAll('a[href*="#/agents/"]').length).toBe(2);
  expect(root.querySelector('a[href*="#/controls/"]')).not.toBeNull();
  await pick(h, root, "Kind family", "invocation");
  expect(root.querySelectorAll("article")).toHaveLength(1);
  await pick(h, root, "Agent filter", agent.name);
  expect(root.querySelectorAll("article")).toHaveLength(1);
  expect(root.querySelector('a[href*="#/agents/"]')?.textContent).toBe(agent.name);
});

test("start sheet reads models and workspaces, completes and validates path, shows git, creates agent and task", async () => {
  h = await connected({ agents: [agent], script });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent"); await h.settle();
  expect(h.daemon.commands("list_models")).toHaveLength(1);
  expect(h.daemon.commands("list_workspaces")).toHaveLength(1);
  await type(h, document.body, "Objective", "Build it");
  await type(h, document.body, "Name", "Builder");
  await type(h, document.body, "Workspace", "/re");
  await debounce();
  expect(h.daemon.commands("complete_path").at(-1)).toMatchObject({ prefix: "/re", kind: "directory", limit: 50 });
  const suggestion = [...document.querySelectorAll<HTMLElement>('[role="option"]')].find(o => o.textContent === "/repo");
  expect(suggestion).toBeDefined();
  await act(async () => suggestion?.click());
  await debounce();
  expect(h.daemon.commands("validate_workspace").at(-1)).toMatchObject({ path: "/repo" });
  expect(h.daemon.commands("get_git_summary")[0]).toMatchObject({ workspace: "/repo" });
  expect(document.body.textContent).toContain("Ready · main · clean");
  await click(h, document.body, "high");
  await click(h, document.body, "Start agent");
  expect(h.daemon.commands("create_agent")[0]).toMatchObject({ name: "Builder", objective: "Build it", role: "worker", start: true, scope: { workspace_root: "/repo" }, model: { provider: "codex", model: "astra" }, config: { effort: "high" }, approval: { require_for: ["external"] } });
  expect(document.querySelector("dialog[open]")).toBeNull();
  expect(location.hash).toContain(agent.id);
  await click(h, root, "Start an agent");
  await click(h, document.body, "Task for an existing agent");
  await type(h, document.body, "Objective", "Build it");
  await pick(h, document.body, "Owner", agent.name);
  await click(h, document.body, "Create task");
  expect(h.daemon.commands("create_task")[0]).toMatchObject({ objective: "Build it", owner: agent.id, priority: "normal" });
});

test("observer start operations are disabled with reasons", async () => {
  h = await connected({ role: "observer", agents: [agent], script });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  for (const label of ["Start agent"]) expect(buttonNamed(document.body, label).disabled).toBe(true);
  expect(document.body.textContent).toContain("Observer devices cannot change daemon state");
  await click(h, document.body, "Task for an existing agent");
  expect(buttonNamed(document.body, "Create task").disabled).toBe(true);
  expect(h.daemon.commands("validate_workspace")).toHaveLength(0);
});

test("lost start reply stays unconfirmed without resubmission", async () => {
  h = await connected({ agents: [agent], script: { ...script, create_task: () => "hold" } });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  await click(h, document.body, "Task for an existing agent");
  await type(h, document.body, "Objective", "Work");
  await pick(h, document.body, "Owner", agent.name);
  await click(h, document.body, "Create task");
  h.daemon.drop(); await h.settle();
  expect(document.body.querySelector('[data-state="unconfirmed"]')).not.toBeNull();
  expect(h.daemon.commands("create_task")).toHaveLength(1);
});

test("large activity uses the core virtual list", async () => {
  h = await connected();
  const root = await h.mount(<ActivityScreen />);
  await act(async () => { for (let i = 0; i < 220; i += 1) h.daemon.emit(agent.id, { type: "invocation.completed" }); });
  await h.settle();
  expect(root.querySelector('.ui-vlist')).not.toBeNull();
  expect(root.querySelectorAll("article").length).toBeLessThan(200);
});

test("history retries errors, pages backwards, and stops at an empty page", async () => {
  let fail = true;
  h = await connected({ script: { get_events: c => fail ? { code: "internal", message: "History unavailable" } :
    { tag: "events", data: (c.query as { before: number }).before > 10 ? [event(10)] : [] } } });
  const root = await h.mount(<ActivityScreen />);
  expect(root.textContent).toContain("History unavailable");
  fail = false;
  await click(h, root, "Try again"); await h.settle();
  expect(root.textContent).toContain("agent.ready");
  await click(h, root, "Show older"); await h.settle();
  expect(h.daemon.commands("get_events").at(-1)).toMatchObject({ query: { before: 10 } });
  expect(buttonNamed(root, "Beginning of history").disabled).toBe(true);
});

test("workspace rejection blocks creation and changing the path invalidates prior validation", async () => {
  h = await connected({ script: { ...script, validate_workspace: () => ({ tag: "workspace_validation", data: { valid: false, reason: "Folder unavailable" } }) } });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  await type(h, document.body, "Objective", "Build");
  await type(h, document.body, "Workspace", "/missing");
  await debounce();
  expect(document.body.textContent).toContain("Folder unavailable");
  expect(buttonNamed(document.body, "Start agent").disabled).toBe(true);
  expect(h.daemon.commands("get_git_summary")).toHaveLength(0);
  h.daemon.script = script;
  await type(h, document.body, "Workspace", "/repo");
  await debounce();
  expect(buttonNamed(document.body, "Start agent").disabled).toBe(false);
  await type(h, document.body, "Workspace", "/another");
  expect(buttonNamed(document.body, "Start agent").disabled).toBe(true);
});

async function debounce() {
  await act(async () => { await new Promise(resolve => setTimeout(resolve, 280)); });
  await h.settle(); await h.settle();
}

test("start draft survives unmount, restores every choice, and clears only after success", async () => {
  h = await connected({ agents: [agent], script });
  let root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  await type(h, document.body, "Objective", "Keep this objective");
  await type(h, document.body, "Name", "Draft agent");
  await type(h, document.body, "Workspace", "/repo");
  await pick(h, document.body, "Model", "Astra");
  await click(h, document.body, "max");
  h.unmount();
  h = await connected({ agents: [agent], script });
  root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  expect(document.querySelector<HTMLTextAreaElement>('[data-testid="start-objective"]')?.value).toBe("Keep this objective");
  expect(document.querySelector<HTMLInputElement>('[data-testid="start-name"]')?.value).toBe("Draft agent");
  expect(buttonNamed(document.body, "max").getAttribute("aria-checked")).toBe("true");
  await debounce();
  await click(h, document.body, "Start agent");
  expect(h.daemon.commands("create_agent")[0]).toMatchObject({ objective: "Keep this objective", name: "Draft agent", config: { effort: "max" } });
  await click(h, root, "Start an agent");
  expect(document.querySelector<HTMLTextAreaElement>('[data-testid="start-objective"]')?.value).toBe("");
});

test("task owner and priority selections preserve ids and protocol priority", async () => {
  h = await connected({ agents: [agent], script });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  await click(h, document.body, "Task for an existing agent");
  await type(h, document.body, "Objective", "Urgent review");
  await pick(h, document.body, "Owner", agent.name);
  await click(h, document.body, "critical");
  await click(h, document.body, "Create task");
  expect(h.daemon.commands("create_task")[0]).toMatchObject({ objective: "Urgent review", owner: agent.id, priority: "critical" });
});

test("an agent that already owns a task is listed but cannot be chosen as the owner", async () => {
  const busy = fauxAgent("agt_busy", { name: "busy", current_task: "task_held" });
  h = await connected({ agents: [agent, busy], script });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  await click(h, document.body, "Task for an existing agent");
  await type(h, document.body, "Objective", "Urgent review");
  const trigger = document.querySelector<HTMLButtonElement>('[data-testid="start-owner"]');
  await act(async () => trigger?.click());
  const option = [...document.querySelectorAll<HTMLElement>('[role="option"]')].find(o => o.textContent?.includes("busy"));
  expect(option?.getAttribute("aria-disabled")).toBe("true");
  expect(option?.textContent).toContain("Already working on a task");
  await act(async () => trigger?.click());
  const submit = document.querySelector<HTMLButtonElement>('[data-testid="start-submit"]');
  expect(submit?.disabled).toBe(true);
  expect(document.getElementById(submit?.getAttribute("aria-describedby") ?? "")?.textContent).toContain("Choose an agent that has no current task");
  await pick(h, document.body, "Owner", agent.name);
  expect(document.querySelector<HTMLButtonElement>('[data-testid="start-submit"]')?.disabled).toBe(false);
});

test("advanced role and approval selections become the exact create command fields", async () => {
  h = await connected({ script });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  await type(h, document.body, "Objective", "Review");
  await act(async () => document.querySelector("summary")?.click());
  await pick(h, document.body, "Role", "reviewer");
  await pick(h, document.body, "Approval policy", "mutation");
  await debounce();
  await click(h, document.body, "Start agent");
  expect(h.daemon.commands("create_agent")[0]).toMatchObject({ role: "reviewer", approval: { require_for: ["mutation"] } });
});

beforeEach(() => { for (const key of drafts.keys()) drafts.clear(key); });

test("desktop Browse fills the workspace and validates without another action", async () => {
  const pickDirectory = vi.fn(async () => "/picked");
  const desktop = { version: 1, pickDirectory, connect: async () => "bridge", close: async () => {}, getPreference: async () => false } as unknown as DesktopBridge;
  h = await connected({ desktop, role: "local", script: { ...script, validate_workspace: c => ({ tag: "workspace_validation", data: { valid: true, path: c.path } }) } });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  expect(document.querySelector('[data-testid="start-workspace-browse"]')).not.toBeNull();
  await click(h, document.body, "Browse");
  expect(pickDirectory).toHaveBeenCalledWith("workspace");
  expect(document.querySelector<HTMLInputElement>('[data-testid="start-workspace"]')?.value).toBe("/picked");
  await debounce();
  expect(h.daemon.commands("validate_workspace").at(-1)).toMatchObject({ path: "/picked" });
  expect(h.daemon.commands("get_git_summary").at(-1)).toMatchObject({ workspace: "/picked" });
});

test("a failed create keeps the objective and draft for retry", async () => {
  h = await connected({ agents: [agent], script: { ...script, create_task: () => ({ code: "invalid_argument", message: "Owner is unavailable" }) } });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  await click(h, document.body, "Task for an existing agent");
  await type(h, document.body, "Objective", "Keep the work");
  await pick(h, document.body, "Owner", agent.name);
  await click(h, document.body, "Create task");
  expect(document.body.textContent).toContain("Owner is unavailable");
  expect(drafts.get("activity/start-agent")).toMatchObject({ objective: "Keep the work", owner: agent.id });
});
