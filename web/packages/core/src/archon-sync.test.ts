import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Json } from "./guards.ts";
import { ConnectionManager } from "./manager.ts";
import { INITIAL_STATE, Store } from "./store.ts";
import { FakeDaemon, type Answer, type Handler } from "./fixtures/fake-daemon.ts";
import F from "./fixtures/archon-frames.json";

const CONN = "archon:host";

function setup(handler: Handler) {
  const store = new Store(INITIAL_STATE, (flush) => flush());
  const daemon = new FakeDaemon(handler, "ws-archon");
  const manager = new ConnectionManager({ store, host: { kind: "browser", storage: { get: async () => undefined, set: async () => {} } }, clientVersion: "test", random: () => 0 });
  manager.add({ daemon: "archon", transport: daemon.transport, target: "wss://host/archon/ws", label: "host" });
  return { store, daemon, manager };
}

type Overrides = Partial<Record<string, (command: Json) => Answer | undefined>>;

/** An archond speaking protocol 24, unless `hello` is overridden. */
function archond(overrides: Overrides = {}): Handler {
  return (command) => {
    const override = overrides[String(command.cmd)];
    if (override) return override(command);
    switch (command.cmd) {
      case "hello": return { ok: F.welcome_24 };
      case "subscribe_changes": return { ok: F.subscribed_changes, then: command.after === null ? F.changes : [] };
      case "task_graph": return { ok: F.task_graph };
      case "list_item_summaries": return { ok: F.summaries_24 };
      case "feed_history": return { ok: F.feed_history };
      case "subscribe_feed": return { ok: F.subscribed_feed };
      case "get_item": return { ok: { reply: "item", data: { id: command.item } } };
      default: throw new Error(`unscripted ${String(command.cmd)}`);
    }
  };
}

const settle = () => vi.advanceTimersByTimeAsync(0);

beforeEach(() => { vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] }); });
afterEach(() => { vi.useRealTimers(); });

describe("Archon protocol 23 fallback (CONTRACTS.md §5.3.5)", () => {
  const archond23 = archond({
    hello: (command) => (command.protocol_version === 23 ? { ok: F.welcome_23 } : { frames: [F.incompatible], close: 1008 }),
    list_item_summaries: () => ({ ok: F.summaries_23 }),
  });

  it("steps down to protocol 23 after an incompatible greeting, without reporting", async () => {
    const { store, daemon, manager } = setup(archond23);
    manager.start();
    await settle();
    expect(store.get().connections[CONN]?.state).toBe("backoff");
    await vi.advanceTimersByTimeAsync(300);
    const hellos = daemon.commands("hello");
    expect(hellos.map((h) => h.protocol_version)).toEqual([24, 23]);
    expect(hellos[0]?.features).toEqual([]);
    expect("features" in (hellos[1] ?? {})).toBe(false);
    expect(store.get().connections[CONN]).toMatchObject({ state: "ready", protocol: 23, standing: "operator" });
    expect(store.get().reports).toEqual([]);
    expect(daemon.commands("subscribe_changes")).toEqual([]);
    expect(Object.keys(store.get().archon[CONN]?.items ?? {})).toEqual(["qi_1", "qi_2"]);
  });

  it("refetches summaries 350 ms after a feed item", async () => {
    const { store, daemon, manager } = setup(archond23);
    manager.start();
    await vi.advanceTimersByTimeAsync(300);
    expect(daemon.commands("list_item_summaries")).toHaveLength(1);
    daemon.push(F.feed_item);
    daemon.push(F.feed_item);
    expect(store.get().archon[CONN]?.feedSeq).toBe(11);
    expect(store.get().archon[CONN]?.feed).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(349);
    expect(daemon.commands("list_item_summaries")).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(daemon.commands("list_item_summaries")).toHaveLength(2);
  });
});

describe("Archon change cursor (CONTRACTS.md §5.3)", () => {
  it("applies buffered changes only to the snapshots they are newer than", async () => {
    const { store, daemon, manager } = setup(archond());
    manager.start();
    await settle();
    const state = store.get().archon[CONN];
    expect(daemon.commands("subscribe_changes")[0]).toMatchObject({ after: null });
    expect(daemon.sent.map((c) => c.cmd).slice(1, 4)).toEqual(["subscribe_changes", "task_graph", "list_item_summaries"]);
    // 105 is newer than the graph (as_of 100) but already in the summaries (as_of 110).
    expect(state?.items.qi_1?.state).toBe("queued");
    expect(state?.graph?.nodes.qi_1?.state).toBe("dispatching");
    expect(state?.items.qi_2?.state).toBe("succeeded");
    expect(state?.graph?.edges).toEqual([{ from: "qi_1", to: "qi_2", kind: "depends_on", satisfied: false }]);
    expect(state?.changeCursor).toBe(116);
    expect(state?.feedSeq).toBe(10);
    expect(daemon.commands("subscribe_feed")[0]).toMatchObject({ after: 10, verbose: true });
  });

  it("drops a change at or below the cursor", async () => {
    const { store, daemon, manager } = setup(archond());
    manager.start();
    await settle();
    const before = store.get().archon[CONN];
    daemon.push({ ...F.changes[1], change: { ...F.changes[1]?.change, seq: 116 } } as Json);
    expect(store.get().archon[CONN]).toBe(before);
  });

  it("resumes both cursors after a reconnect without rereading snapshots", async () => {
    const { daemon, manager } = setup(archond());
    manager.start();
    await settle();
    daemon.drop(1006);
    await vi.advanceTimersByTimeAsync(300);
    expect(daemon.commands("subscribe_changes").map((c) => c.after)).toEqual([null, 116]);
    expect(daemon.commands("subscribe_feed").map((c) => c.after)).toEqual([10, 10]);
    expect(daemon.commands("task_graph")).toHaveLength(1);
    expect(daemon.commands("feed_history")).toHaveLength(1);
  });

  it("rereads the task graph on history_expired, then subscribes after its as_of", async () => {
    const { store, daemon, manager } = setup(archond({
      subscribe_changes: (command) => (command.after === 116 ? { err: F.history_expired } : { ok: F.subscribed_changes, then: command.after === null ? F.changes : [] }),
    }));
    manager.start();
    await settle();
    daemon.drop(1006);
    await vi.advanceTimersByTimeAsync(300);
    expect(daemon.commands("subscribe_changes").map((c) => c.after)).toEqual([null, 116, 100]);
    expect(daemon.commands("task_graph")).toHaveLength(2);
    expect(store.get().connections[CONN]?.state).toBe("ready");
    expect(store.get().archon[CONN]?.changeCursor).toBe(100);
    expect(store.get().reports).toEqual([]);
  });

  it("refetches an on-screen item once per 250 ms and leaves others in place", async () => {
    const { daemon, manager } = setup(archond());
    manager.start();
    await settle();
    manager.showItems(CONN, ["qi_1"]);
    const change = (n: number, item: string) => ({ frame: "change", sub: 6, change: { seq: n, at: "2026-11-04T03:00:09Z", subject: { subject: "item", item, state: "running" } } });
    daemon.push(change(120, "qi_1"));
    daemon.push(change(121, "qi_1"));
    daemon.push(change(122, "qi_2"));
    await vi.advanceTimersByTimeAsync(250);
    expect(daemon.commands("get_item")).toEqual([expect.objectContaining({ item: "qi_1" })]);
  });

  it("rereads snapshots, keeping the cursor, when a change names an unknown item", async () => {
    const { store, daemon, manager } = setup(archond());
    manager.start();
    await settle();
    daemon.push({ frame: "change", sub: 6, change: { seq: 130, at: "2026-11-04T03:00:09Z", subject: { subject: "item", item: "qi_new", state: "queued" } } });
    await vi.advanceTimersByTimeAsync(250);
    expect(daemon.commands("task_graph")).toHaveLength(2);
    expect(daemon.commands("list_item_summaries")).toHaveLength(2);
    const state = store.get().archon[CONN];
    expect(state?.changeCursor).toBe(130);
    // The reread graph (as_of 100) gets the recent changes replayed onto it.
    expect(state?.graph?.nodes.qi_2?.state).toBe("succeeded");
    expect(state?.graph?.edges).toHaveLength(1);
  });

  it("keeps the on-screen items across a reconnect", async () => {
    const { daemon, manager } = setup(archond());
    manager.start();
    await settle();
    manager.showItems(CONN, ["qi_2"]);
    daemon.drop(1006);
    await vi.advanceTimersByTimeAsync(300);
    daemon.push({ frame: "change", sub: 6, change: { seq: 140, at: "2026-11-04T03:00:09Z", subject: { subject: "item", item: "qi_2", state: "failed" } } });
    await vi.advanceTimersByTimeAsync(250);
    expect(daemon.commands("get_item")).toEqual([expect.objectContaining({ item: "qi_2" })]);
  });
});
