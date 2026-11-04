// @vitest-environment happy-dom
import { afterEach, expect, test, vi } from "vitest";
import type { aether, DesktopBridge } from "@aether-ui/protocol";
import { connected, click, buttonNamed, type, type Harness } from "../controls/fixture/harness.test-util.tsx";
import MemoryScreen from "./MemoryScreen.tsx";
import { MemoryRow } from "./MemoryRow.tsx";
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

test("local proposals accept and confirmed reject use desktop privileged and refresh the store", async () => {
  const privileged = vi.fn(async () => ({ reply: "ok" }));
  const desktop = { version: 1, connect: async () => "bridge", close: async () => {}, privileged, getPreference: async () => false } as unknown as DesktopBridge;
  h = await connected({ role: "local", desktop, script: { memories: c => ({ tag: "memories", data: (c.query as { proposed?: boolean }).proposed ? [memory] : [] }) } });
  const root = await h.mount(<MemoryScreen params={{ conn: h.conn }} />);
  await click(h, root, "Accept");
  await h.settle();
  expect(privileged).toHaveBeenCalledWith({ conn: "bridge", kind: "memory.accept", memory: memory.id });
  await click(h, root, "Reject");
  expect(privileged).toHaveBeenCalledTimes(1);
  await click(h, document.body, "Reject proposal");
  await h.settle();
  expect(privileged).toHaveBeenLastCalledWith({ conn: "bridge", kind: "memory.reject", memory: memory.id });
  expect(h.daemon.commands("memories")).toHaveLength(6);
});

test("lost mutation reply is unconfirmed without retry", async () => {
  h = await connected({ script: { memories: () => ({ tag: "memories", data: [memory] }), pin_memory: () => "hold" } });
  const root = await h.mount(<MemoryScreen params={{ conn: h.conn }} />);
  await click(h, root, "Pin");
  h.daemon.drop(); await h.settle();
  expect(root.querySelector('[data-state="unconfirmed"]')).not.toBeNull();
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

test("scope and kind filters and forgotten toggle use exact query fields", async () => {
  h = await connected({ script: { memories: () => ({ tag: "memories", data: [] }) } });
  const root = await h.mount(<MemoryScreen params={{ conn: h.conn }} />);
  await click(h, root, "Global");
  expect(h.daemon.commands("memories").at(-1)).toMatchObject({ query: { scope: { scope: "global" } } });
  await click(h, root, "Repository");
  await type(h, root, "Repository filter", "/repo");
  await click(h, root, "decision");
  const checkbox = root.querySelector('input[type="checkbox"]') as HTMLInputElement;
  await import("react").then(({ act }) => act(async () => checkbox.click()));
  await h.settle();
  expect(h.daemon.commands("memories").at(-1)).toMatchObject({ query: { scope: { scope: "repository", id: "/repo" }, kinds: ["decision"], include_forgotten: true, limit: 100 } });
});
