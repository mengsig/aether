// @vitest-environment happy-dom
import { afterEach, expect, test, vi } from "vitest";
import { act } from "react";
import { connectionDraftKey, drafts, routeHref } from "@aether-ui/core";
import type { DesktopBridge } from "@aether-ui/protocol";
import type { aether } from "@aether-ui/protocol";
import { connected, click, buttonNamed, pick, type, type Harness } from "../controls/fixture/harness.test-util.tsx";
import { fauxAgent } from "../conversation/fixture/transcript.ts";
import { invariant } from "../fleet/invariant.ts";
import ActivityScreen from "./ActivityScreen.tsx";
import StartScreen from "./StartScreen.tsx";
import { FIRST_MESSAGE_LIMIT } from "./firstMessage.ts";
import { UndeliveredFirst } from "./UndeliveredFirst.tsx";
let h: Harness;
afterEach(() => {
  h?.unmount();
  location.hash = "";
  for (const key of drafts.keys()) drafts.clear(key);
});
const agent = fauxAgent();
const event = (seq: number): aether.Event => ({ id: `evt_${seq}`, seq, at: "2026-11-04", actor: { actor: "system" }, agent_id: agent.id, kind: { type: "agent.ready" } });
const script = {
  list_models: () => ({ tag: "models", data: [{ name: "Astra", configured: true, is_default: true, model: { provider: "codex", model: "astra" } }] }),
  list_workspaces: () => ({ tag: "workspaces", data: [{ path: "/repo", exists: true }] }),
  validate_workspace: () => ({ tag: "workspace_validation", data: { valid: true, path: "/repo" } }),
  status: () => ({ tag: "status", data: { pid: 7, instance_id: "b3", network_default: "allowed" } }),
  get_git_summary: () => ({ tag: "git_summary", data: { repository: true, branch: "main", dirty: false } }),
  complete_path: () => ({ tag: "path_completions", data: { entries: [{ path: "/repo", name: "repo" }], truncated: false } }),
  create_agent: () => ({ tag: "agent", data: agent }),
  send_message: () => ({ tag: "message_sent", data: { seq: 1 } }),
  create_task: () => ({ tag: "task", data: { id: "task_1" } }),
};

test("shows live events, backward store pages, filters, and agent links", async () => {
  h = await connected({ agents: [agent], events: [event(20)], script: { get_events: () => ({ tag: "events", data: [event(10)] }) } });
  const root = await h.mount(<ActivityScreen />);
  await act(async () => { h.daemon.emit(agent.id, { type: "invocation.completed" }); }); await h.settle();
  expect(root.textContent).toContain("Invocation completed");
  expect(h.daemon.commands("get_events")[0]).toMatchObject({ after: 0, limit: 100, query: { before: Number.MAX_SAFE_INTEGER } });
  expect(h.app.store.get().reads[h.conn]?.[`activity:before:${Number.MAX_SAFE_INTEGER}`]?.value).toEqual([event(10)]);
  await click(h, root, "Show older"); await h.settle();
  expect(h.daemon.commands("get_events").at(-1)).toMatchObject({ after: 0, limit: 100, query: { before: 10 } });
  expect(root.querySelectorAll('a[href*="#/agents/"]').length).toBe(2);
  expect(root.querySelector('a[href*="#/controls/"]')).not.toBeNull();
  await pick(h, root, "Kind family", "Invocation");
  expect(root.querySelectorAll("article")).toHaveLength(1);
  await pick(h, root, "Agent filter", agent.name);
  expect(root.querySelectorAll("article")).toHaveLength(1);
  expect(root.querySelector('a[href*="#/agents/"]')?.textContent).toBe(agent.name);
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
  await click(h, root, "Start an agent"); await h.settle();
  expect(document.activeElement).toBe(field("First message"));
  await click(h, document.body, "Task agent"); await h.settle(); await h.settle();
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
  expect(document.body.textContent).toContain("Start agent");
});

test("starting an agent sends the form, closes the sheet, opens the new agent and reports success", async () => {
  h = await connected({ agents: [agent], script: startScript });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent"); await click(h, document.body, "Task agent"); await h.settle();
  await type(h, document.body, "What should the agent do?", "Build it");
  await type(h, document.body, "Name", "Builder");
  await type(h, document.body, "Effort", "high");
  await type(h, document.body, "Model", "claude/sonnet");
  await choose("old-repo");
  await click(h, document.body, "Start agent"); await h.settle();
  expect(h.daemon.commands("create_agent")[0]).toMatchObject({ name: "Builder", objective: "Build it", role: "worker", start: true, scope: { workspace_root: "/work/old-repo", network: "allowed" }, model: { provider: "claude", model: "sonnet" }, config: { effort: "high" }, approval: { require_for: ["external"] } });
  expect(document.body.querySelector("dialog")).toBeNull();
  expect(location.hash).toBe(routeHref("#/agents/:conn/:agent", { conn: h.conn, agent: agent.id }));
  expect(h.app.store.get().reports.at(-1)).toMatchObject({ message: `Started ${agent.name}`, tone: "success", conn: h.conn });
});

test("UX-043: a started agent keeps the daemon's network default, so Archon's tools reach it", async () => {
  h = await connected({ agents: [agent], script: { ...startScript, status: () => "hold" } });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent"); await click(h, document.body, "Task agent"); await h.settle();
  await type(h, document.body, "What should the agent do?", "Build it");
  expect(buttonNamed(document.body, "Start agent").disabled).toBe(true);
  expect(document.body.textContent).toContain("Reading this computer's settings…");
  h.unmount();
  h = await connected({ agents: [agent], script: { ...startScript, status: () => ({ tag: "status", data: { pid: 7, instance_id: "b3", network_default: "denied" } }) } });
  await click(h, await h.mount(<ActivityScreen />), "Start an agent"); await click(h, document.body, "Task agent"); await h.settle();
  await type(h, document.body, "What should the agent do?", "Build it");
  await click(h, document.body, "Start agent"); await h.settle();
  expect(h.daemon.commands("create_agent")[0]).toMatchObject({ scope: { workspace_root: "/work/repo", network: "denied" } });
});

test("a start in flight cannot be submitted twice", async () => {
  h = await connected({ agents: [agent], script: { ...startScript, create_agent: () => "hold" } });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent"); await click(h, document.body, "Task agent"); await h.settle();
  await type(h, document.body, "What should the agent do?", "Build it");
  await click(h, document.body, "Start agent");
  expect(buttonNamed(document.body, "Start agent").disabled).toBe(true);
  expect(h.daemon.commands("create_agent")).toHaveLength(1);
  expect(document.body.querySelector("dialog")).not.toBeNull();
});

test("a task goes to its owner agent with a confirmation", async () => {
  h = await connected({ agents: [agent], script: startScript });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent"); await click(h, document.body, "Task agent");
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
  await click(h, root, "Start an agent"); await click(h, document.body, "Task agent"); await h.settle(); await h.settle();
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
  await click(h, root, "Start an agent"); await click(h, document.body, "Task agent"); await h.settle(); await h.settle();
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

/** Validates /repo in the open start sheet, as a person picks a workspace. */
async function pickRepo(): Promise<void> {
  await h.settle(); await h.settle();
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

/** Starts a chat with `text` through the sheet, as a person does. */
async function startChat(text: string): Promise<void> {
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  await type(h, document.body, "First message", text);
  await pickRepo();
  await click(h, document.body, "Start chat"); await h.settle();
}

const refusedOnce = () => {
  let refusals = 1;
  return () => (refusals-- > 0 ? { code: "invalid_request", message: "Inbox is full" } : { tag: "message_sent", data: { seq: 1 } });
};
const sheet = (): HTMLElement => {
  const dialog = document.body.querySelector("dialog");
  invariant(dialog instanceof HTMLElement, "the start sheet is open");
  return dialog;
};
const sent = () => h.daemon.commands("send_message").map((c) => [c.to, c.content]);
const chatHash = () => `#/agents/${encodeURIComponent(h.conn)}/${agent.id}`;

test("a refused first message keeps the sheet, shows the chat's settings fixed, and resends the saved text to the same chat", async () => {
  window.location.hash = "";
  h = await connected({ agents: [agent], script: { ...script, send_message: refusedOnce() } });
  await startChat("/calm please");
  expect(document.body.textContent).toContain("The first message was not delivered: Inbox is full");
  expect(window.location.hash).not.toContain(agent.id);
  const settings = document.body.querySelector('[aria-label="Chat settings, fixed once the chat exists"]');
  expect(settings?.textContent, "the settings it was created with are shown").toContain("/repo");
  expect(settings?.textContent).toContain("Astra");
  expect(sheet().querySelectorAll("input, textarea"), "nothing about a created chat is editable").toHaveLength(0);
  expect(document.body.textContent).not.toContain("Task agent");
  expect(document.body.querySelector('[aria-label="First message, as typed"]')?.textContent).toBe("/calm please");
  await click(h, document.body, "Send it again"); await h.settle();
  expect(h.daemon.commands("create_agent"), "a retry never creates another chat").toHaveLength(1);
  expect(sent(), "sent as typed, never read as a command").toEqual([[agent.id, "/calm please"], [agent.id, "/calm please"]]);
  expect(window.location.hash).toBe(chatHash());
  expect(sessionStorage.length, "a delivered first message leaves nothing for the chat").toBe(0);
});

test("a new chat with different settings is an explicit action that keeps the form and creates once more", async () => {
  window.location.hash = "";
  h = await connected({ agents: [agent], script: { ...script, send_message: () => ({ code: "invalid_request", message: "Inbox is full" }) } });
  await startChat("Hello");
  expect(buttonNamed(document.body, "New chat with different settings")).not.toBeNull();
  await click(h, document.body, "New chat with different settings");
  expect(field("Effort"), "its settings are offered again").not.toBeNull();
  expect((sheet().querySelector("textarea") as HTMLTextAreaElement | null)?.value, "the form keeps what was typed").toBe("Hello");
  await click(h, document.body, "Start chat"); await h.settle();
  expect(h.daemon.commands("create_agent")).toHaveLength(2);
});

test("a first message whose reply is lost says it may have arrived and retries only on the same chat", async () => {
  h = await connected({ agents: [agent], script: { ...script, send_message: () => "hold" } });
  await startChat("Hello");
  h.daemon.drop();
  await h.settle();
  expect(document.body.querySelector('[data-state="unconfirmed"]')?.textContent).toContain("it may have arrived");
  expect(document.body.querySelector('a[href="#/inbox"]'), "the way to the recovery review").not.toBeNull();
  expect(buttonNamed(document.body, "Send it again"), "a retry goes to the same chat").not.toBeNull();
  expect(h.daemon.commands("create_agent")).toHaveLength(1);
});

test("a create whose reply is lost never creates a second chat unless asked to", async () => {
  h = await connected({ agents: [agent], script: { ...script, create_agent: () => "hold" } });
  await startChat("Hello");
  h.daemon.drop();
  await h.settle();
  expect(document.body.textContent).toContain("it is not known whether the chat was created");
  expect(sheet().querySelector("textarea"), "the sheet cannot start the same chat again").toBeNull();
  expect(h.daemon.commands("send_message")).toHaveLength(0);
  await click(h, document.body, "New chat with different settings");
  expect(buttonNamed(document.body, "Start chat")).not.toBeNull();
  expect(h.daemon.commands("create_agent")).toHaveLength(1);
});

test("a first message too long to keep is refused before any chat is created", async () => {
  h = await connected({ agents: [agent], script });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  await type(h, document.body, "First message", "x".repeat(FIRST_MESSAGE_LIMIT + 1));
  await pickRepo();
  expect(buttonNamed(document.body, "Start chat").disabled).toBe(true);
  expect(document.body.textContent).toContain("The first message is too long");
  expect(h.daemon.commands("create_agent")).toHaveLength(0);
});

test("the chat it opens carries a refused first message and sends it as typed from there", async () => {
  window.location.hash = "";
  h = await connected({ agents: [agent], script: { ...script, send_message: refusedOnce() } });
  await startChat("/calm please");
  const link = [...document.body.querySelectorAll("a")].find((a) => a.textContent === "Open the chat");
  expect(link?.getAttribute("href")).toBe(chatHash());
  await act(async () => link?.click()); await h.settle();
  expect(document.body.querySelector('[aria-label="Started chat"]'), "opening the chat closes the sheet").toBeNull();
  const chat = await h.mount(<UndeliveredFirst conn={h.conn} agent={agent.id} />);
  expect(chat.textContent).toContain("The first message was not delivered: Inbox is full");
  expect(chat.querySelector('[aria-label="First message, as typed"]')?.textContent).toBe("/calm please");
  await click(h, chat, "Send it as typed"); await h.settle();
  expect(sent()).toEqual([[agent.id, "/calm please"], [agent.id, "/calm please"]]);
  expect(chat.textContent, "a delivered message is no longer shown as undelivered").toBe("");
});

test("a store that cannot keep the outcome is said in the sheet, which still never creates again", async () => {
  h = await connected({ agents: [agent], script: { ...script, send_message: () => ({ code: "invalid_request", message: "Inbox is full" }) } });
  const setItem = vi.spyOn(sessionStorage, "setItem").mockImplementation(() => { throw new DOMException("full", "QuotaExceededError"); });
  try {
    await startChat("Hello");
  } finally {
    setItem.mockRestore();
  }
  expect(document.body.textContent).toContain("The chat cannot be told about it");
  expect(sheet().querySelector("input")).toBeNull();
  expect(buttonNamed(document.body, "Send it again")).not.toBeNull();
  expect(h.daemon.commands("create_agent")).toHaveLength(1);
});

test("a store that cannot clear the kept copy still opens the chat a delivered first message started", async () => {
  window.location.hash = "";
  h = await connected({ agents: [agent], script: { ...script, send_message: () => ({ tag: "message_sent", data: { seq: 1 } }) } });
  const removeItem = vi.spyOn(sessionStorage, "removeItem").mockImplementation(() => { throw new DOMException("denied", "SecurityError"); });
  try {
    await startChat("Hello");
  } finally {
    removeItem.mockRestore();
  }
  expect(window.location.hash, "the delivered start is finished").toBe(chatHash());
  expect(h.app.store.get().reports.map((r) => r.message)).toContain("The first message's kept copy could not be cleared (denied); the chat may offer it again.");
  expect(h.daemon.commands("create_agent")).toHaveLength(1);
});


test("observer start operations are disabled with reasons", async () => {
  h = await connected({ role: "observer", agents: [agent], script: startScript });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent"); await click(h, document.body, "Task agent");
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
  await act(async () => (document.body.querySelector<HTMLButtonElement>('[aria-label="Close Start an agent"]') as HTMLButtonElement).click());
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
  expect(root.textContent).toContain("Agent ready");
  await click(h, root, "Show older"); await h.settle();
  expect(h.daemon.commands("get_events").at(-1)).toMatchObject({ query: { before: 10 } });
  expect(buttonNamed(root, "Beginning of history").disabled).toBe(true);
});

test("workspace rejection blocks creation and choosing another workspace validates it", async () => {
  const verdict = (c: Record<string, unknown>) => c.path === "/work/repo" ?
    { tag: "workspace_validation", data: { valid: false, reason: "Folder unavailable" } } : { tag: "workspace_validation", data: { valid: true, path: c.path } };
  h = await connected({ script: { ...startScript, validate_workspace: verdict } });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent"); await click(h, document.body, "Task agent"); await h.settle(); await h.settle();
  await type(h, document.body, "What should the agent do?", "Build");
  expect(document.body.textContent).toContain("Folder unavailable");
  expect(buttonNamed(document.body, "Start agent").disabled).toBe(true);
  expect(h.daemon.commands("get_git_summary")).toHaveLength(0);
  await choose("old-repo");
  expect(buttonNamed(document.body, "Start agent").disabled).toBe(false);
});

test("Start a task opens the Start sheet at once, and closing it leaves the start route", async () => {
  h = await connected({ script: startScript });
  location.hash = "#/activity/start";
  const root = await h.mount(<StartScreen />);
  expect(root.querySelector("dialog[open]") ?? document.querySelector("dialog[open]")).not.toBeNull();
  await act(async () => (document.body.querySelector<HTMLButtonElement>('[aria-label="Close Start an agent"]') as HTMLButtonElement).click());
  await h.settle();
  expect(document.querySelector("dialog[open]")).toBeNull();
  expect(location.hash).toBe("#/activity");
});

test("the start draft survives unmount per connection, restores every choice, and clears once the agent exists", async () => {
  h = await connected({ agents: [agent], script: startScript });
  let root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent"); await click(h, document.body, "Task agent"); await h.settle();
  await type(h, document.body, "What should the agent do?", "Keep this objective");
  await type(h, document.body, "Name", "Draft agent");
  await type(h, document.body, "Effort", "max");
  await choose("old-repo");
  const key = connectionDraftKey("activity", h.conn, "start-agent");
  expect(drafts.get(key)).toMatchObject({ mode: "agent", objective: "Keep this objective", name: "Draft agent", effort: "max", path: "/work/old-repo" });
  h.unmount();
  h = await connected({ agents: [agent], script: startScript, instance: "laptop" });
  root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent");
  expect(document.querySelector<HTMLTextAreaElement>('[data-testid="start-objective"]')?.value, "another connection starts blank").toBe("");
  h.unmount();
  h = await connected({ agents: [agent], script: startScript });
  root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent"); await h.settle(); await h.settle();
  expect(document.querySelector<HTMLTextAreaElement>('[data-testid="start-objective"]')?.value).toBe("Keep this objective");
  expect((field("Name") as HTMLInputElement).value).toBe("Draft agent");
  expect((field("Effort") as HTMLSelectElement).value).toBe("max");
  expect(choice("old-repo")?.querySelector("input")?.checked).toBe(true);
  await click(h, document.body, "Start agent"); await h.settle();
  expect(h.daemon.commands("create_agent")[0]).toMatchObject({ objective: "Keep this objective", name: "Draft agent", config: { effort: "max" }, scope: { workspace_root: "/work/old-repo" } });
  expect(drafts.get(key)).toBeUndefined();
});

test("a failed create keeps the draft for retry", async () => {
  h = await connected({ agents: [agent], script: { ...startScript, create_task: () => ({ code: "invalid_argument", message: "Owner is unavailable" }) } });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent"); await click(h, document.body, "Existing agent task");
  await type(h, document.body, "What should the agent do?", "Keep the work");
  await type(h, document.body, "Agent", agent.id);
  await click(h, document.body, "Create task"); await h.settle();
  expect(document.body.textContent).toContain("Owner is unavailable");
  expect(drafts.get(connectionDraftKey("activity", h.conn, "start-agent"))).toMatchObject({ objective: "Keep the work", owner: agent.id });
});

test("desktop Browse fills another folder and validates it without another action", async () => {
  const pickDirectory = vi.fn(async () => "/picked");
  const desktop = { version: 1, pickDirectory, connect: async () => "bridge", close: async () => {}, getPreference: async () => false } as unknown as DesktopBridge;
  h = await connected({ desktop, role: "local", agents: [agent], script: startScript });
  const root = await h.mount(<ActivityScreen />);
  await click(h, root, "Start an agent"); await click(h, document.body, "Task agent"); await h.settle();
  await choose("Other folder");
  await click(h, document.body, "Browse");
  expect(pickDirectory).toHaveBeenCalledWith("workspace");
  expect(document.querySelector<HTMLInputElement>('[data-testid="start-workspace-path"]')?.value).toBe("/picked");
  expect(h.daemon.commands("validate_workspace").at(-1)).toMatchObject({ path: "/picked" });
  expect(h.daemon.commands("get_git_summary").at(-1)).toMatchObject({ workspace: "/picked" });
});
