// @vitest-environment happy-dom
import { afterEach, expect, test } from "vitest";
import { act } from "react";
import type { aether } from "@aether-ui/protocol";
import { connected, click, buttonNamed, type, type Harness } from "../controls/fixture/harness.test-util.tsx";
import { fauxAgent } from "../conversation/fixture/transcript.ts";
import { draftKey, readDraft } from "../composer/drafts.ts";
import ActivityScreen from "./ActivityScreen.tsx";
let h: Harness;
afterEach(() => h?.unmount());
const agent = fauxAgent();
const event = (seq: number): aether.Event => ({ id: `evt_${seq}`, seq, at: "2026-11-04", actor: { actor: "system" }, agent_id: agent.id, kind: { type: "agent.ready" } });
const script = {
  list_models: () => ({ tag: "models", data: [{ name: "Astra", configured: true, is_default: true, model: { provider: "codex", model: "astra" } }] }),
  list_workspaces: () => ({ tag: "workspaces", data: [{ path: "/repo", exists: true }] }),
  validate_workspace: () => ({ tag: "workspace_validation", data: { valid: true, path: "/repo" } }),
  get_git_summary: () => ({ tag: "git_summary", data: { repository: true, branch: "main", dirty: false } }),
  complete_path: () => ({ tag: "path_completions", data: { entries: [{ path: "/repo", name: "repo" }], truncated: false } }),
  create_agent: () => ({ tag: "agent", data: agent }),
  create_task: () => ({ tag: "task", data: { id: "task_1" } }),
  send_message: () => ({ tag: "message_sent", data: { seq: 1 } }),
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
  await type(h, root, "Kind family", "invocation");
  expect(root.querySelectorAll("article")).toHaveLength(1);
  await type(h, root, "Agent filter", "other");
  expect(root.textContent).toContain("No activity to show");
});

test("start sheet reads models and workspaces, completes and validates path, shows git, creates agent and task", async () => {
  h = await connected({ agents: [agent], script });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent"); await h.settle();
  expect(h.daemon.commands("list_models")).toHaveLength(1);
  expect(h.daemon.commands("list_workspaces")).toHaveLength(1);
  await click(h, document.body, "Task agent");
  await type(h, document.body, "Objective", "Build it");
  await type(h, document.body, "Name", "Builder");
  await type(h, document.body, "Workspace", "/re");
  await click(h, document.body, "Complete path"); await h.settle();
  expect(h.daemon.commands("complete_path")[0]).toMatchObject({ prefix: "/re", kind: "directory", limit: 50 });
  await click(h, document.body, "/repo");
  await click(h, document.body, "Validate workspace"); await h.settle(); await h.settle();
  expect(h.daemon.commands("validate_workspace")[0]).toMatchObject({ path: "/repo" });
  expect(h.daemon.commands("get_git_summary")[0]).toMatchObject({ workspace: "/repo" });
  expect(document.body.textContent).toContain("main · Clean");
  await click(h, document.body, "high");
  await click(h, document.body, "Start agent");
  expect(h.daemon.commands("create_agent")[0]).toMatchObject({ name: "Builder", objective: "Build it", role: "worker", start: true, scope: { workspace_root: "/repo" }, model: { provider: "codex", model: "astra" }, config: { effort: "high" }, approval: { require_for: ["external"] } });
  expect(document.body.textContent).toContain("Done");
  await click(h, document.body, "Existing agent task");
  await click(h, document.body, agent.name);
  await click(h, document.body, "Create task");
  expect(h.daemon.commands("create_task")[0]).toMatchObject({ objective: "Build it", owner: agent.id, priority: "normal" });
});

/** Validates /repo in the open start sheet, as a person picks a workspace. */
async function pickRepo(): Promise<void> {
  await type(h, document.body, "Workspace", "/repo");
  await click(h, document.body, "Validate workspace"); await h.settle(); await h.settle();
}

test("a chat is the default start: a task-less interactive assistant, then its first message, then its conversation", async () => {
  h = await connected({ agents: [agent], script });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent"); await h.settle();
  expect(document.body.textContent).toContain("A chat stays open");
  await type(h, document.body, "First message", "  What changed in the parser?  ");
  await pickRepo();
  await click(h, document.body, "Start chat"); await h.settle();
  const [create] = h.daemon.commands("create_agent");
  expect(create).toMatchObject({ role: "assistant", name: "assistant", start: true, scope: { workspace_root: "/repo" }, config: { effort: "medium", interactive: true } });
  expect(create).not.toHaveProperty("objective");
  expect(h.daemon.commands("send_message")).toEqual([expect.objectContaining({ cmd: "send_message", to: agent.id, content: "What changed in the parser?" })]);
  expect(window.location.hash).toBe(`#/agents/${encodeURIComponent(h.conn)}/${agent.id}`);
});

test("a chat whose create fails sends no message", async () => {
  h = await connected({ agents: [agent], script: { ...script, create_agent: () => ({ code: "invalid_request", message: "No such model" }) } });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  await type(h, document.body, "First message", "Hello");
  await pickRepo();
  await click(h, document.body, "Start chat"); await h.settle();
  expect(document.body.textContent).toContain("No such model");
  expect(h.daemon.commands("send_message")).toHaveLength(0);
});

test("a chat whose first message fails opens with that message as its draft, and is never created twice", async () => {
  h = await connected({ agents: [agent], script: { ...script, send_message: () => ({ code: "invalid_request", message: "Inbox is full" }) } });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  await type(h, document.body, "First message", "  Hello there  ");
  await pickRepo();
  await click(h, document.body, "Start chat"); await h.settle();
  expect(h.daemon.commands("create_agent")).toHaveLength(1);
  expect(h.daemon.commands("send_message")).toHaveLength(1);
  expect(window.location.hash).toBe(`#/agents/${encodeURIComponent(h.conn)}/${agent.id}`);
  expect(readDraft(draftKey(h.conn, agent.id, "message"))).toBe("Hello there");
  expect(document.body.textContent, "the start sheet closed").not.toContain("A chat stays open");
});

test("a delivered first message leaves no draft behind", async () => {
  h = await connected({ agents: [agent], script });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  await type(h, document.body, "First message", "Hello");
  await pickRepo();
  await click(h, document.body, "Start chat"); await h.settle();
  expect(readDraft(draftKey(h.conn, agent.id, "message"))).toBe("");
});

test("observer start operations are disabled with reasons", async () => {
  h = await connected({ role: "observer", agents: [agent], script });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  for (const label of ["Start chat", "Complete path", "Validate workspace"]) expect(buttonNamed(document.body, label).disabled).toBe(true);
  expect(document.body.textContent).toContain("Observer devices cannot change daemon state");
  await click(h, document.body, "Existing agent task");
  expect(buttonNamed(document.body, "Create task").disabled).toBe(true);
  expect(h.daemon.commands("validate_workspace")).toHaveLength(0);
});

test("lost start reply stays unconfirmed without resubmission", async () => {
  h = await connected({ agents: [agent], script: { ...script, create_task: () => "hold" } });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  await click(h, document.body, "Existing agent task");
  await type(h, document.body, "Objective", "Work");
  await click(h, document.body, agent.name);
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
  await type(h, document.body, "First message", "Build");
  await type(h, document.body, "Workspace", "/missing");
  await click(h, document.body, "Validate workspace"); await h.settle();
  expect(document.body.textContent).toContain("Folder unavailable");
  expect(buttonNamed(document.body, "Start chat").disabled).toBe(true);
  expect(h.daemon.commands("get_git_summary")).toHaveLength(0);
  h.daemon.script = script;
  await type(h, document.body, "Workspace", "/repo");
  await click(h, document.body, "Validate workspace"); await h.settle();
  expect(buttonNamed(document.body, "Start chat").disabled).toBe(false);
  await type(h, document.body, "Workspace", "/another");
  expect(buttonNamed(document.body, "Start chat").disabled).toBe(true);
});
