// @vitest-environment happy-dom
import { drafts } from "@aether-ui/core";
import { beforeEach, afterEach, expect, test, vi } from "vitest";
import type { aether, DesktopBridge } from "@aether-ui/protocol";
import { connected, click, buttonNamed, pick, type, type Harness } from "../controls/fixture/harness.test-util.tsx";
import { AGENT_ID, demoAgents } from "../controls/fixture/demo.ts";
import MemoryScreen from "./MemoryScreen.tsx";
import { MemoryRow } from "./MemoryRow.tsx";
import { V2_FEATURES } from "./fixture/data.ts";
let h: Harness;
afterEach(() => h?.unmount());
const memory: aether.MemoryRecord = { id: "mem_1", text: "Remember the invariant", kind: "fact", scope: { scope: "global" }, tags: ["design"], created_at: "2026-11-04", confidence: 1, importance: 1, provenance: {} };

test("lists stored memories, filters, creates, pins, confirms forget, and restores with rereads", async () => {
  let row = memory;
  h = await connected({ script: {
    memories: () => ({ tag: "memories", data: [row] }),
    create_memory: c => { row = { ...row, ...(c.spec as object) }; return { tag: "memory", data: row }; },
    pin_memory: () => { row = { ...row, pinned: true }; return { tag: "ok", data: null }; },
    forget_memory: () => { row = { ...row, forgotten_at: "today" }; return { tag: "ok", data: null }; },
    restore_memory: () => { row = { ...row, forgotten_at: null }; return { tag: "ok", data: null }; },
  } });
  const root = await h.mount(<MemoryScreen params={{ conn: h.conn }} />);
  expect(root.textContent).toContain(memory.text);
  await type(h, root, "Search memories", "invariant");
  expect(h.daemon.commands("memories").at(-1)).toMatchObject({ query: { text: "invariant", limit: 100 } });
  await click(h, root, "Create memory");
  await type(h, document.body, "Memory text", "New memory");
  await type(h, document.body, "Tags", "one, two");
  await click(h, document.body, "Save memory");
  expect(h.daemon.commands("create_memory")[0]).toMatchObject({ spec: { text: "New memory", kind: "fact", scope: { scope: "global" }, tags: ["one", "two"] } });
  await h.settle();
  expect(root.textContent).toContain("New memory");
  await click(h, root, "Pin");
  expect(h.daemon.commands("pin_memory")[0]).toMatchObject({ id: memory.id, pinned: true });
  await h.settle();
  expect(root.textContent).toContain("Pinned");
  await click(h, root, "Forget");
  expect(h.daemon.commands("forget_memory")).toHaveLength(0);
  await click(h, document.body, "Forget memory");
  await h.settle();
  expect(h.daemon.commands("forget_memory")[0]).toMatchObject({ id: memory.id });
  await click(h, root, "Restore");
  await h.settle();
  expect(h.daemon.commands("restore_memory")[0]).toMatchObject({ id: memory.id });
  expect(h.app.store.get().reads[h.conn]?.["memory:list"]?.value).toEqual([row]);
  expect(h.daemon.commands("memories").length).toBeGreaterThanOrEqual(6);
});

test("observer controls are refused and proposals are never read remotely", async () => {
  h = await connected({ role: "observer", script: { memories: () => ({ tag: "memories", data: [memory] }) } });
  const root = await h.mount(<MemoryScreen params={{ conn: h.conn }} />);
  expect(buttonNamed(root, "Pin").disabled).toBe(true);
  expect(buttonNamed(root, "Forget").disabled).toBe(true);
  expect(root.textContent).toContain("Observer devices cannot change daemon state");
  expect(h.daemon.commands("memories").every(c => !(c.query as { proposed?: boolean }).proposed)).toBe(true);
  await click(h, root, "Create memory");
  expect(buttonNamed(document.body, "Save memory").disabled).toBe(true);
});

test("remote proposals actions display host-only marks and refusal", async () => {
  h = await connected();
  const root = await h.mount(<MemoryRow conn={h.conn} memory={memory} proposal refresh={async () => {}} />);
  expect(buttonNamed(root, "Accept").disabled).toBe(true);
  expect(buttonNamed(root, "Reject").disabled).toBe(true);
  expect(root.textContent).toContain("Host only");
  expect(root.textContent).toContain("Only the daemon's own host can do this");
});

test("on the host the proposals listing is available", async () => {
  const desktop = { version: 1, connect: async () => "bridge", close: async () => {}, privileged: vi.fn(), getPreference: async () => false } as unknown as DesktopBridge;
  h = await connected({ role: "local", desktop, script: { memories: () => ({ tag: "memories", data: [] }) } });
  const root = await h.mount(<MemoryScreen params={{ conn: h.conn }} />);
  expect(h.daemon.commands("memories").some(c => (c.query as { proposed?: boolean }).proposed)).toBe(true);
  expect(root.textContent).toContain("No proposals to review");
});

test("a proposal row accepts and, after confirming, rejects through desktop privileged", async () => {
  const privileged = vi.fn(async () => ({ reply: "ok" }));
  const desktop = { version: 1, connect: async () => "bridge", close: async () => {}, privileged, getPreference: async () => false } as unknown as DesktopBridge;
  h = await connected({ role: "local", desktop });
  const refresh = vi.fn(async () => {});
  const root = await h.mount(<MemoryRow conn={h.conn} memory={memory} proposal refresh={refresh} />);
  await click(h, root, "Accept");
  await h.settle();
  expect(privileged).toHaveBeenCalledWith({ conn: "bridge", kind: "memory.accept", memory: memory.id });
  await click(h, root, "Reject");
  expect(privileged).toHaveBeenCalledTimes(1);
  await click(h, document.body, "Reject proposal");
  await h.settle();
  expect(privileged).toHaveBeenLastCalledWith({ conn: "bridge", kind: "memory.reject", memory: memory.id });
  expect(refresh).toHaveBeenCalledTimes(2);
});

test("a blank search omits text, and a repository scope is named by its id", async () => {
  const scoped = { ...memory, scope: { scope: "repository", id: "/srv/repo" } } as aether.MemoryRecord;
  h = await connected({ script: { memories: () => ({ tag: "memories", data: [scoped] }) } });
  const root = await h.mount(<MemoryScreen params={{ conn: h.conn }} />);
  expect(h.daemon.commands("memories")[0]?.query).not.toHaveProperty("text");
  expect(root.textContent).toContain("Repository /srv/repo");
  await type(h, root, "Search memories", "  invariant ");
  expect(h.daemon.commands("memories").at(-1)).toMatchObject({ query: { text: "invariant" } });
});

test("a lost unkeyed pin reaches the recovery review and is never retried", async () => {
  h = await connected({ script: { memories: () => ({ tag: "memories", data: [memory] }), pin_memory: () => "hold" } });
  const root = await h.mount(<MemoryScreen params={{ conn: h.conn }} />);
  await click(h, root, "Pin");
  h.daemon.drop(); await h.settle();
  expect(root.querySelector('[data-state="unconfirmed"]')?.textContent).toContain("It waits in the recovery review");
  expect(h.app.store.get().aether[h.conn]?.outbox).toEqual([expect.objectContaining({ unkeyed: true, command: expect.objectContaining({ cmd: "pin_memory" }) })]);
  expect(h.daemon.commands("pin_memory")).toHaveLength(1);
});

test("failed list read exposes retry and successful stored empty state", async () => {
  h = await connected({ script: { memories: () => ({ code: "internal", message: "Unavailable" }) } });
  const root = await h.mount(<MemoryScreen params={{ conn: h.conn }} />);
  expect(root.textContent).toContain("Unavailable");
  h.daemon.script = { memories: () => ({ tag: "memories", data: [] }) };
  await click(h, root, "Try again"); await h.settle();
  expect(root.textContent).toContain("No memories yet");
});

test("a late answer to an earlier search never hides the newer search's failure", async () => {
  h = await connected({ script: { memories: (command) => (command.query as { text?: string }).text ? { code: "internal", message: "Search failed" } : "hold" } });
  const root = await h.mount(<MemoryScreen params={{ conn: h.conn }} />);
  await type(h, root, "Search memories", "invariant");
  expect(root.textContent).toContain("Search failed");
  h.daemon.script = { memories: () => ({ tag: "memories", data: [memory] }) };
  h.daemon.release(); await h.settle();
  expect(root.textContent).toContain("Search failed");
  expect(root.querySelector('[aria-busy="true"]')).toBeNull();
  expect(buttonNamed(root, "Try again").disabled).toBe(false);
});

test("scope and kind filters and forgotten toggle use exact query fields", async () => {
  h = await connected({ script: { memories: () => ({ tag: "memories", data: [{ ...memory, scope: { scope: "repository", id: "/repo" } }] }) } });
  const root = await h.mount(<MemoryScreen params={{ conn: h.conn }} />);
  await click(h, root, "Global");
  expect(h.daemon.commands("memories").at(-1)).toMatchObject({ query: { scope: { scope: "global" } } });
  await click(h, root, "Repository");
  expect(h.daemon.commands("memories").every(c => (c.query as { scope?: { id?: string } }).scope?.id !== "")).toBe(true);
  await pick(h, root, "Repository filter", "Another repository…");
  await type(h, root, "Repository filter", "/repo");
  await click(h, root, "decision");
  await click(h, root, "Include forgotten");
  await h.settle();
  expect(h.daemon.commands("memories").at(-1)).toMatchObject({ query: { scope: { scope: "repository", id: "/repo" }, kinds: ["decision"], include_forgotten: true, limit: 100 } });
  await type(h, root, "Repository filter", "github.com/example/older-repo");
  expect(h.daemon.commands("memories").at(-1)).toMatchObject({ query: { scope: { scope: "repository", id: "github.com/example/older-repo" } } });
});

test("repository creation uses the operator root and refreshes the store count", async () => {
  let total = 0;
  h = await connected({ features: V2_FEATURES, script: {
    memories: () => ({ tag: "memories", data: [] }),
    create_memory: () => { total++; return { tag: "memory", data: memory }; },
    memory_migration_status: () => ({ tag: "memory_migration_status", data: {
      schema_version: 32, migrations: [], memories: { total, admitted: total, implicit: 0, candidates: 0, stale: 0, held_out: 0 }, backups: [],
    } }),
  } });
  const root = await h.mount(<MemoryScreen params={{ conn: h.conn }} />);
  await click(h, root, "Create memory");
  await type(h, document.body, "Memory text", "A repository fact");
  await click(h, document.querySelector('[aria-label="Memory scope"]') as HTMLElement, "Repository");
  await type(h, document.body, "Repository root", "/repo");
  await click(h, document.body, "Save memory");
  await h.settle();
  expect(h.daemon.commands("create_memory")[0]).toMatchObject({ spec: { scope: { scope: "repository", root: "/repo" } } });
  expect(h.daemon.commands("memory_migration_status")).toHaveLength(2);
  expect(root.querySelector(".mem-big")?.textContent).toBe("1");
});

test("a rejected memory can be discarded or corrected and retried", async () => {
  let reject = true;
  h = await connected({ script: { memories: () => ({ tag: "memories", data: [] }), create_memory: () => reject
    ? { code: "invalid_request", message: "Memory text is too short" } : { tag: "memory", data: memory } } });
  const root = await h.mount(<MemoryScreen params={{ conn: h.conn }} />);
  await click(h, root, "Create memory");
  await type(h, document.body, "Memory text", "x");
  await import("react").then(({ act }) => act(async () => (document.querySelector('[aria-label="Close Create memory"]') as HTMLButtonElement).click()));
  await click(h, root, "Create memory");
  expect((document.querySelector('dialog textarea') as HTMLInputElement).value).toBe("x");
  await click(h, document.body, "Save memory");
  expect(document.body.textContent).toContain("Memory text is too short");
  await click(h, document.body, "Discard");
  await click(h, root, "Create memory");
  expect((document.querySelector('dialog textarea') as HTMLInputElement).value).toBe("");
  await type(h, document.body, "Memory text", "A corrected memory");
  reject = false;
  await click(h, document.body, "Save memory");
  expect(h.daemon.commands("create_memory")).toHaveLength(2);
});

test("an agent-scoped memory is created, filtered and shown by its agent", async () => {
  const scoped = { ...memory, scope: { scope: "agent", id: AGENT_ID } } as aether.MemoryRecord;
  h = await connected({ agents: demoAgents(), script: { memories: () => ({ tag: "memories", data: [scoped] }), create_memory: () => ({ tag: "memory", data: scoped }) } });
  const root = await h.mount(<MemoryScreen params={{ conn: h.conn }} />);
  expect(root.textContent).toContain(`Agent ${AGENT_ID}`);
  await click(h, root, "Agent");
  await pick(h, root, "Agent filter", demoAgents()[0]?.name ?? "");
  expect(h.daemon.commands("memories").at(-1)).toMatchObject({ query: { scope: { scope: "agent", id: AGENT_ID } } });
  await click(h, root, "Create memory");
  await type(h, document.body, "Memory text", "Agent note");
  await click(h, document.querySelector('[aria-label="Memory scope"]') as HTMLElement, "Agent");
  expect(buttonNamed(document.body, "Save memory").disabled).toBe(true);
  await pick(h, document.body, "Agent", demoAgents()[0]?.name ?? "");
  await click(h, document.body, "Save memory");
  expect(h.daemon.commands("create_memory")[0]).toMatchObject({ spec: { text: "Agent note", scope: { scope: "agent", id: AGENT_ID } } });
});

test("repository suggestions combine agent roots, recent workspaces and recorded scopes", async () => {
  const scoped = { ...memory, scope: { scope: "repository", id: "/recorded" } } as aether.MemoryRecord;
  h = await connected({ agents: demoAgents().map(a => ({ ...a, scope: { workspace_root: "/agent-root" } })), script: {
    memories: () => ({ tag: "memories", data: [scoped] }),
    list_workspaces: () => ({ tag: "workspaces", data: [{ path: "/recent", exists: true }] }),
    create_memory: () => ({ tag: "memory", data: scoped }),
  } });
  const root = await h.mount(<MemoryScreen params={{ conn: h.conn }} />);
  await click(h, root, "Repository");
  await pick(h, root, "Repository filter", "/recent");
  expect(h.daemon.commands("memories").at(-1)).toMatchObject({ query: { scope: { scope: "repository", id: "/recent" } } });
  await pick(h, root, "Repository filter", "All");
  expect(h.daemon.commands("memories").at(-1)).toMatchObject({ query: { scope: null } });
  await click(h, root, "Create memory");
  await click(h, document.querySelector('[aria-label="Memory scope"]') as HTMLElement, "Repository");
  await type(h, document.body, "Memory text", "Repository lesson");
  await click(h, document.querySelector('[aria-label="Memory kind"]') as HTMLElement, "lesson");
  await type(h, document.body, "Repository root", "/agent");
  const option = [...document.querySelectorAll<HTMLElement>('[role="option"]')].find(o => o.textContent === "/agent-root");
  expect(option).toBeDefined();
  await import("react").then(({ act }) => act(async () => option?.click()));
  await type(h, document.body, "Tags", "one, two");
  expect(document.querySelectorAll("dialog .ui-badge").length).toBeGreaterThanOrEqual(2);
  await click(h, document.body, "Save memory");
  expect(h.daemon.commands("create_memory")[0]).toMatchObject({ spec: { kind: "lesson", text: "Repository lesson", scope: { scope: "repository", root: "/agent-root" }, tags: ["one", "two"] } });
});

beforeEach(() => { for (const key of drafts.keys()) drafts.clear(key); });
