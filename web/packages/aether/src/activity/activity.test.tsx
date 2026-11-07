// @vitest-environment happy-dom
import { afterEach, expect, test } from "vitest";
import { act } from "react";
import { routeHref } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { connected, click, buttonNamed, type, type Harness } from "../controls/fixture/harness.test-util.tsx";
import { fauxAgent } from "../conversation/fixture/transcript.ts";
import { invariant } from "../fleet/invariant.ts";
import ActivityScreen from "./ActivityScreen.tsx";
let h: Harness;
afterEach(() => { h?.unmount(); location.hash = ""; });
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

const workspaces = [
  { path: "/work/old-repo", exists: true, active_at: "2026-11-01T10:00:00Z" },
  { path: "/work/repo", exists: true, active_at: "2026-11-05T10:00:00Z" },
  { path: "/gone/missing", exists: false, active_at: "2026-11-06T10:00:00Z" },
];
const models = [
  { name: "Claude Sonnet", configured: true, is_default: false, provider_name: "Anthropic", model: { provider: "claude", model: "sonnet" } },
  { name: "Astra", configured: true, is_default: true, provider_name: "OpenAI", model: { provider: "codex", model: "astra" } },
  { name: "Unready", configured: false, is_default: false, provider_name: "Other", model: { provider: "other", model: "unready" } },
];
const startScript = {
  ...script,
  list_models: () => ({ tag: "models", data: models }),
  list_workspaces: () => ({ tag: "workspaces", data: workspaces }),
  validate_workspace: (c: Record<string, unknown>) => ({ tag: "workspace_validation", data: { valid: true, path: c.path } }),
};

const field = (label: string): HTMLElement => {
  const found = [...document.body.querySelectorAll("label")].find(l => (l.querySelector(".ui-field-label")?.textContent ?? l.textContent) === label);
  invariant(found, `a field named ${label}`);
  return (found.htmlFor ? document.getElementById(found.htmlFor) : found.querySelector("textarea, select, input")) as HTMLElement;
};
const options = (label: string) => [...field(label).querySelectorAll("option")].map(o => o.textContent);
const choice = (name: string) => [...document.body.querySelectorAll<HTMLElement>(".start-choice")].find(c => c.querySelector(".start-choice-name")?.textContent === name);
async function choose(name: string) {
  const radio = choice(name)?.querySelector("input");
  invariant(radio, `a workspace choice named ${name}`);
  await act(async () => radio.click());
  await h.settle(); await h.settle();
}

test("start sheet is a guided form: focused objective, ready models, human effort, preselected validated workspace, one footer action", async () => {
  h = await connected({ agents: [agent], script: startScript });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent"); await h.settle(); await h.settle();
  expect(document.activeElement).toBe(field("What should the agent do?"));
  expect(field("What should the agent do?").tagName).toBe("TEXTAREA");
  expect(options("Model")).toEqual(["Claude Sonnet", "Astra"]);
  expect([...document.body.querySelectorAll("optgroup")].map(g => g.label)).toEqual(["Anthropic", "OpenAI"]);
  expect((field("Model") as HTMLSelectElement).value).toBe("codex/astra");
  expect(options("Effort")).toEqual(["Low", "Medium", "High", "Extra high", "Max"]);
  expect((field("Effort") as HTMLSelectElement).value).toBe("medium");
  expect(choice("repo")?.querySelector("input")?.checked).toBe(true);
  expect(choice("missing")).toBeUndefined();
  expect(h.daemon.commands("validate_workspace")[0]).toMatchObject({ path: "/work/repo" });
  expect(h.daemon.commands("get_git_summary")[0]).toMatchObject({ workspace: "/work/repo" });
  expect(document.body.textContent).toContain("main · Clean");
  expect([...document.body.querySelectorAll("button")].some(b => b.textContent === "Validate workspace")).toBe(false);
  expect(document.body.querySelector(".ui-dialog-footer")?.textContent).toContain("Start agent");
});

test("starting an agent sends the form, closes the sheet, opens the new agent and reports success", async () => {
  h = await connected({ agents: [agent], script: startScript });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent"); await h.settle();
  await type(h, document.body, "What should the agent do?", "Build it");
  await type(h, document.body, "Name", "Builder");
  await type(h, document.body, "Effort", "high");
  await type(h, document.body, "Model", "claude/sonnet");
  await choose("old-repo");
  await click(h, document.body, "Start agent"); await h.settle();
  expect(h.daemon.commands("create_agent")[0]).toMatchObject({ name: "Builder", objective: "Build it", role: "worker", start: true, scope: { workspace_root: "/work/old-repo" }, model: { provider: "claude", model: "sonnet" }, config: { effort: "high" }, approval: { require_for: ["external"] } });
  expect(document.body.querySelector("dialog")).toBeNull();
  expect(location.hash).toBe(routeHref("#/agents/:conn/:agent", { conn: h.conn, agent: agent.id }));
  expect(h.app.store.get().reports.at(-1)).toMatchObject({ message: `Started ${agent.name}`, tone: "success", conn: h.conn });
});

test("a start in flight cannot be submitted twice", async () => {
  h = await connected({ agents: [agent], script: { ...startScript, create_agent: () => "hold" } });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent"); await h.settle();
  await type(h, document.body, "What should the agent do?", "Build it");
  await click(h, document.body, "Start agent");
  expect(buttonNamed(document.body, "Start agent").disabled).toBe(true);
  expect(h.daemon.commands("create_agent")).toHaveLength(1);
  expect(document.body.querySelector("dialog")).not.toBeNull();
});

test("a task goes to its owner agent with a confirmation", async () => {
  h = await connected({ agents: [agent], script: startScript });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  await click(h, document.body, "Existing agent task");
  await type(h, document.body, "What should the agent do?", "Build it");
  await type(h, document.body, "Agent", agent.id);
  await click(h, document.body, "Create task"); await h.settle();
  expect(h.daemon.commands("create_task")[0]).toMatchObject({ objective: "Build it", owner: agent.id, priority: "normal" });
  expect(document.body.querySelector("dialog")).toBeNull();
  expect(location.hash).toBe(routeHref("#/agents/:conn/:agent", { conn: h.conn, agent: agent.id }));
  expect(h.app.store.get().reports.at(-1)).toMatchObject({ message: `Started task for ${agent.name}`, tone: "success" });
});

test("a workspace choice shows the folder name over a truncated parent path, with the full path in the title", async () => {
  const deep = "/home/someone/projects/clients/a-very-long-organisation-name/another-long-segment/aether-web-frontend";
  h = await connected({ script: { ...startScript, list_workspaces: () => ({ tag: "workspaces", data: [{ path: deep, exists: true, active_at: "2026-11-05T10:00:00Z" }] }) } });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent"); await h.settle(); await h.settle();
  const item = choice("aether-web-frontend");
  expect(item?.title).toBe(deep);
  const parent = item?.querySelector<HTMLElement>(".start-choice-path");
  expect(parent?.textContent).toBe(deep.slice(0, deep.lastIndexOf("/")));
  expect(parent?.title).toBe(deep);
  expect(item?.querySelector(".start-choice-name")?.textContent).toBe("aether-web-frontend");
  expect([...document.body.querySelectorAll("button")].map(b => b.textContent)).not.toContain(deep);
});

test("another folder can be typed or completed, and is validated on commit", async () => {
  h = await connected({ script: startScript });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent"); await h.settle(); await h.settle();
  await choose("Other folder");
  expect(buttonNamed(document.body, "Start agent").disabled).toBe(true);
  await type(h, document.body, "Folder path", "/re");
  await click(h, document.body, "Complete path"); await h.settle();
  expect(h.daemon.commands("complete_path")[0]).toMatchObject({ prefix: "/re", kind: "directory", limit: 50 });
  await act(async () => (field("Folder path") as HTMLInputElement).dispatchEvent(new FocusEvent("focusout", { bubbles: true })));
  await h.settle(); await h.settle();
  expect(h.daemon.commands("validate_workspace").at(-1)).toMatchObject({ path: "/re" });
  await choose("repo");
  expect(h.daemon.commands("validate_workspace").at(-1)).toMatchObject({ path: "/work/repo" });
  expect(buttonNamed(document.body, "Start agent").disabled).toBe(true);
});

test("observer start operations are disabled with reasons", async () => {
  h = await connected({ role: "observer", agents: [agent], script: startScript });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  expect(buttonNamed(document.body, "Start agent").disabled).toBe(true);
  expect(document.body.textContent).toContain("Observer devices cannot change daemon state");
  expect((field("What should the agent do?") as HTMLTextAreaElement).disabled).toBe(true);
  await click(h, document.body, "Existing agent task");
  expect(buttonNamed(document.body, "Create task").disabled).toBe(true);
  expect(h.daemon.commands("validate_workspace")).toHaveLength(0);
});

test("closing the sheet returns focus to the Start an agent button", async () => {
  h = await connected({ script: startScript });
  const root = await h.mount(<ActivityScreen />);
  const opener = buttonNamed(root, "Start an agent");
  opener.focus();
  await click(h, root, "Start an agent");
  expect(document.activeElement).not.toBe(opener);
  await act(async () => buttonNamed(document.body, "✕").click());
  await h.settle();
  expect(document.activeElement).toBe(opener);
});

test("lost start reply stays unconfirmed without resubmission", async () => {
  h = await connected({ agents: [agent], script: { ...startScript, create_task: () => "hold" } });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  await click(h, document.body, "Existing agent task");
  await type(h, document.body, "What should the agent do?", "Work");
  await type(h, document.body, "Agent", agent.id);
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

test("workspace rejection blocks creation and choosing another workspace validates it", async () => {
  const verdict = (c: Record<string, unknown>) => c.path === "/work/repo" ?
    { tag: "workspace_validation", data: { valid: false, reason: "Folder unavailable" } } : { tag: "workspace_validation", data: { valid: true, path: c.path } };
  h = await connected({ script: { ...startScript, validate_workspace: verdict } });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent"); await h.settle(); await h.settle();
  await type(h, document.body, "What should the agent do?", "Build");
  expect(document.body.textContent).toContain("Folder unavailable");
  expect(buttonNamed(document.body, "Start agent").disabled).toBe(true);
  expect(h.daemon.commands("get_git_summary")).toHaveLength(0);
  await choose("old-repo");
  expect(buttonNamed(document.body, "Start agent").disabled).toBe(false);
});
