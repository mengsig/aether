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

describe("Archon protocol 23 fallback retries", () => {
  it("refetches summaries again after a fallback refresh times out", async () => {
    const { store, daemon, manager } = setup(archond({
      hello: (command) => (command.protocol_version === 23 ? { ok: F.welcome_23 } : { frames: [F.incompatible], close: 1008 }),
      list_item_summaries: () => (daemon.commands("list_item_summaries").length === 2 ? undefined : { ok: F.summaries_23 }),
    }));
    manager.start();
    await vi.advanceTimersByTimeAsync(300);
    daemon.push(F.feed_item);
    await vi.advanceTimersByTimeAsync(350 + 20_000);
    expect(store.get().reports).toEqual([expect.objectContaining({ code: "timeout" })]);
    await vi.advanceTimersByTimeAsync(350);
    expect(daemon.commands("list_item_summaries")).toHaveLength(3);
    expect(store.get().connections[CONN]?.state).toBe("ready");
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
    const created = { id: "qi_new", state: "queued", title: "new" };
    const graphAt125 = { ...F.task_graph, data: { ...F.task_graph.data, nodes: [...F.task_graph.data.nodes, created], as_of: 125 } };
    const { store, daemon, manager } = setup(archond({
      task_graph: () => {
        if (daemon.commands("task_graph").length === 1) return { ok: F.task_graph };
        // A change committed after the reread's as_of arrives before its answer.
        daemon.push({ frame: "change", sub: 6, change: { seq: 131, at: "2026-11-04T03:00:10Z", subject: { subject: "item", item: "qi_new", state: "dispatching" } } });
        return { ok: graphAt125 };
      },
    }));
    manager.start();
    await settle();
    daemon.push({ frame: "change", sub: 6, change: { seq: 130, at: "2026-11-04T03:00:09Z", subject: { subject: "item", item: "qi_new", state: "queued" } } });
    await vi.advanceTimersByTimeAsync(250);
    expect(daemon.commands("task_graph")).toHaveLength(2);
    expect(daemon.commands("list_item_summaries")).toHaveLength(2);
    const state = store.get().archon[CONN];
    expect(state?.changeCursor).toBe(131);
    expect(state?.graph?.nodes.qi_new?.state).toBe("dispatching");
    expect(state?.graph?.asOf).toBe(125);
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

describe("Archon resync, second review", () => {
  const change = (n: number, item: string, state: string) => ({ frame: "change", sub: 6, change: { seq: n, at: "2026-11-04T03:00:09Z", subject: { subject: "item", item, state } } });

  it("rereads snapshots after a reconnect when an unknown item's change preceded it", async () => {
    const { daemon, manager } = setup(archond());
    manager.start();
    await settle();
    daemon.push(change(130, "qi_new", "queued"));
    daemon.drop(1006);
    await vi.advanceTimersByTimeAsync(300);
    expect(daemon.commands("subscribe_changes").map((c) => c.after)).toEqual([null, 130]);
    await vi.advanceTimersByTimeAsync(250);
    expect(daemon.commands("task_graph")).toHaveLength(2);
  });

  it("refetches an on-screen item after a reconnect when its change preceded it", async () => {
    const { daemon, manager } = setup(archond());
    manager.start();
    await settle();
    manager.showItems(CONN, ["qi_1"]);
    daemon.push(change(130, "qi_1", "running"));
    daemon.drop(1006);
    await vi.advanceTimersByTimeAsync(300 + 250);
    expect(daemon.commands("get_item")).toEqual([expect.objectContaining({ item: "qi_1" })]);
  });

  it("replays collected changes onto the graph even when the summaries reread fails", async () => {
    const graphAt125 = { ...F.task_graph, data: { ...F.task_graph.data, as_of: 125 } };
    const { store, daemon, manager } = setup(archond({
      task_graph: () => {
        if (daemon.commands("task_graph").length === 1) return { ok: F.task_graph };
        daemon.push(change(131, "qi_1", "failed"));
        return { ok: graphAt125 };
      },
      list_item_summaries: () => (daemon.commands("list_item_summaries").length === 1 ? { ok: F.summaries_24 } : { err: { code: "internal", message: "boom" } }),
    }));
    manager.start();
    await settle();
    daemon.push(change(130, "qi_new", "queued"));
    await vi.advanceTimersByTimeAsync(250);
    const state = store.get().archon[CONN];
    expect(state?.graph?.asOf).toBe(125);
    expect(state?.graph?.nodes.qi_1?.state).toBe("failed");
    expect(store.get().reports).toEqual([expect.objectContaining({ message: expect.stringContaining("boom") })]);
  });
});

describe("Archon resync, third review", () => {
  const initiative = (n: number, id: string, state: string) => ({ frame: "change", sub: 6, change: { seq: n, at: "2026-11-04T03:00:09Z", subject: { subject: "initiative", initiative: id, state } } });
  const withInitiative = { ...F.task_graph, data: { ...F.task_graph.data, initiatives: [{ id: "in_1", title: "one", state: "active", atomic: false }] } };

  it("applies an initiative change to the graph it is newer than", async () => {
    const { store, daemon, manager } = setup(archond({ task_graph: () => ({ ok: withInitiative }) }));
    manager.start();
    // The recorded changes add an edge whose satisfaction only a reread can tell.
    await vi.advanceTimersByTimeAsync(250);
    daemon.push(initiative(130, "in_1", "completed"));
    await vi.advanceTimersByTimeAsync(250);
    expect(store.get().archon[CONN]?.graph?.initiatives).toEqual([{ id: "in_1", title: "one", state: "completed", atomic: false }]);
    expect(daemon.commands("task_graph")).toHaveLength(2);
  });

  it("rereads the snapshots when a change names an unknown initiative", async () => {
    const { store, daemon, manager } = setup(archond({ task_graph: () => ({ ok: withInitiative }) }));
    manager.start();
    await settle();
    daemon.push(initiative(130, "in_new", "active"));
    expect(store.get().archon[CONN]?.staleSnapshots).toBe(true);
    await vi.advanceTimersByTimeAsync(250);
    expect(daemon.commands("task_graph")).toHaveLength(2);
    expect(store.get().archon[CONN]?.changeCursor).toBe(130);
  });

  it("moves a graph node's stage and waiting reason with an item change", async () => {
    const waiting = { id: "qi_1", state: "waiting", state_detail: "approval of build", stage: "build", waiting_reason: "approval", title: "item qi_1" };
    const graph = { ...F.task_graph, data: { ...F.task_graph.data, nodes: [waiting, F.task_graph.data.nodes[1]] } };
    const { store, daemon, manager } = setup(archond({ task_graph: () => ({ ok: graph }) }));
    manager.start();
    await settle();
    daemon.push({ frame: "change", sub: 6, change: { seq: 130, at: "2026-11-04T03:00:09Z", subject: { subject: "item", item: "qi_1", state: "running", stage: "test" } } });
    expect(store.get().archon[CONN]?.graph?.nodes.qi_1).toEqual({ id: "qi_1", state: "running", stage: "test", title: "item qi_1" });
    expect(store.get().archon[CONN]?.items.qi_1).toMatchObject({ state: "running", stage: "test" });
  });

  it("refetches on-screen items after history_expired, since skipped changes moved them", async () => {
    const { store, daemon, manager } = setup(archond({
      subscribe_changes: (command) => (command.after === 130 ? { err: F.history_expired } : { ok: F.subscribed_changes, then: command.after === null ? F.changes : [] }),
    }));
    manager.start();
    await settle();
    manager.showItems(CONN, ["qi_1"]);
    daemon.push({ frame: "change", sub: 6, change: { seq: 130, at: "2026-11-04T03:00:09Z", subject: { subject: "item", item: "qi_1", state: "waiting" } } });
    await vi.advanceTimersByTimeAsync(250);
    expect(store.get().archon[CONN]?.details.qi_1).toEqual({ id: "qi_1" });
    daemon.drop(1006);
    await vi.advanceTimersByTimeAsync(300 + 250);
    expect(daemon.commands("subscribe_changes").map((c) => c.after)).toEqual([null, 130, 100]);
    expect(daemon.commands("get_item")).toHaveLength(2);
    expect(store.get().archon[CONN]?.staleItems).toEqual([]);
  });

  it("refreshes an item's detail when it returns to the screen after an off-screen change", async () => {
    const { store, daemon, manager } = setup(archond());
    manager.start();
    await settle();
    manager.showItems(CONN, ["qi_1"]);
    await vi.advanceTimersByTimeAsync(250);
    expect(daemon.commands("get_item")).toHaveLength(1);
    manager.showItems(CONN, []);
    daemon.push({ frame: "change", sub: 6, change: { seq: 130, at: "2026-11-04T03:00:09Z", subject: { subject: "item", item: "qi_1", state: "succeeded" } } });
    expect(store.get().archon[CONN]?.details.qi_1).toBeUndefined();
    manager.showItems(CONN, ["qi_1"]);
    await vi.advanceTimersByTimeAsync(250);
    expect(daemon.commands("get_item")).toHaveLength(2);
    expect(store.get().archon[CONN]?.details.qi_1).toEqual({ id: "qi_1" });
  });

  it("does not install a detail read that an off-screen change overtook", async () => {
    let hold = true;
    const { store, daemon, manager } = setup(archond({ get_item: () => (hold ? undefined : { ok: { reply: "item", data: { id: "qi_1", state: "succeeded" } } }) }));
    manager.start();
    await settle();
    manager.showItems(CONN, ["qi_1"]);
    await vi.advanceTimersByTimeAsync(250);
    const pending = daemon.commands("get_item")[0];
    manager.showItems(CONN, []);
    daemon.push({ frame: "change", sub: 6, change: { seq: 130, at: "2026-11-04T03:00:09Z", subject: { subject: "item", item: "qi_1", state: "succeeded" } } });
    daemon.push({ frame: "reply", id: pending?.req, result: { Ok: { reply: "item", data: { id: "qi_1", state: "running" } } } });
    await settle();
    expect(store.get().archon[CONN]?.details.qi_1).toBeUndefined();
    hold = false;
    manager.showItems(CONN, ["qi_1"]);
    await vi.advanceTimersByTimeAsync(250);
    expect(daemon.commands("get_item")).toHaveLength(2);
    expect(store.get().archon[CONN]?.details.qi_1).toEqual({ id: "qi_1", state: "succeeded" });
  });

  it("keeps a feed replay that arrives with the subscribe answer", async () => {
    const replayed = { ...F.feed_item, item: { ...F.feed_item.item, seq: 11 } };
    const { store, daemon, manager } = setup(archond({ subscribe_feed: (command) => ({ ok: F.subscribed_feed, then: command.after === 10 && daemon.commands("subscribe_feed").length === 2 ? [replayed] : [] }) }));
    manager.start();
    await settle();
    daemon.drop(1006);
    await vi.advanceTimersByTimeAsync(300);
    daemon.push({ ...F.feed_item, item: { ...F.feed_item.item, seq: 12 } });
    expect(store.get().archon[CONN]?.feed.map((item) => (item as Json).seq).slice(-2)).toEqual([11, 12]);
    expect(store.get().archon[CONN]?.feedSeq).toBe(12);
  });

  it("rereads the graph when a prerequisite moves, since archond derives edge satisfaction", async () => {
    const edge = { from: "qi_1", to: "qi_2", kind: "depends_on", satisfied: false };
    const graph = (satisfied: boolean, asOf: number) => ({ ...F.task_graph, data: { ...F.task_graph.data, edges: [{ ...edge, satisfied }], as_of: asOf } });
    const { store, daemon, manager } = setup(archond({ subscribe_changes: () => ({ ok: F.subscribed_changes }), task_graph: () => ({ ok: daemon.commands("task_graph").length === 1 ? graph(false, 100) : graph(true, 130) }) }));
    manager.start();
    await settle();
    daemon.push({ frame: "change", sub: 6, change: { seq: 120, at: "2026-11-04T03:00:09Z", subject: { subject: "item", item: "qi_2", state: "running" } } });
    await vi.advanceTimersByTimeAsync(250);
    expect(daemon.commands("task_graph")).toHaveLength(1);
    daemon.push({ frame: "change", sub: 6, change: { seq: 130, at: "2026-11-04T03:00:09Z", subject: { subject: "item", item: "qi_1", state: "succeeded" } } });
    expect(store.get().archon[CONN]?.staleSnapshots).toBe(true);
    await vi.advanceTimersByTimeAsync(250);
    expect(daemon.commands("task_graph")).toHaveLength(2);
    expect(store.get().archon[CONN]?.graph?.edges).toEqual([{ ...edge, satisfied: true }]);
  });

  it("retries a snapshot reread that timed out on a live link, and reports it", async () => {
    const { store, daemon, manager } = setup(archond({
      subscribe_changes: () => ({ ok: F.subscribed_changes }),
      task_graph: () => (daemon.commands("task_graph").length === 2 ? undefined : { ok: F.task_graph }),
    }));
    manager.start();
    await settle();
    daemon.push({ frame: "change", sub: 6, change: { seq: 130, at: "2026-11-04T03:00:09Z", subject: { subject: "item", item: "qi_new", state: "queued" } } });
    await vi.advanceTimersByTimeAsync(250 + 20_000);
    expect(store.get().archon[CONN]?.staleSnapshots).toBe(true);
    expect(store.get().reports).toEqual([expect.objectContaining({ code: "timeout" })]);
    await vi.advanceTimersByTimeAsync(250);
    expect(daemon.commands("task_graph")).toHaveLength(3);
    expect(store.get().connections[CONN]?.state).toBe("ready");
  });
});

