import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { checkSearch, HISTORY_LIMIT, SEARCHES_KEPT, searchKey, withStamped } from "./archon-reads.ts";
import { GRAPH_LIMIT } from "./archon-sync.ts";
import type { Json } from "./guards.ts";
import { ConnectionManager } from "./manager.ts";
import { INITIAL_STATE, Store } from "./store.ts";
import { FakeDaemon, type Answer } from "./fixtures/fake-daemon.ts";
import F from "./fixtures/archon-frames.json";

const CONN = "archon:host";
const WELCOME = { ...F.welcome_24, data: { ...F.welcome_24.data, features: [...F.welcome_24.data.features, "item_history", "item_filters"] } };
const change = (seq: number, item: string): Json => ({ frame: "change", sub: 6, change: { seq, at: "2026-11-04T03:00:09Z", subject: "item", item, state: "running" } });
const transitions = (...seqs: number[]): Json => ({ reply: "item_history", data: { item: "qi_1", transitions: seqs.map((seq) => ({ seq, at: "2026-11-04T03:00:00Z", from: null, to: "queued", by: "cli" })) } });

type Overrides = Partial<Record<string, (command: Json, daemon: FakeDaemon) => Answer | undefined>>;

function setup(overrides: Overrides = {}, welcome: Json = WELCOME) {
  const store = new Store(INITIAL_STATE, (flush) => flush());
  const daemon = new FakeDaemon((command, self) => {
    const override = overrides[String(command.cmd)];
    if (override) return override(command, self);
    switch (command.cmd) {
      case "hello": return { ok: welcome };
      case "subscribe_changes": return { ok: F.subscribed_changes };
      case "task_graph": return { ok: F.task_graph };
      case "list_item_summaries": return { ok: F.summaries_24 };
      case "feed_history": return { ok: F.feed_history };
      case "subscribe_feed": return { ok: F.subscribed_feed };
      default: throw new Error(`unscripted ${String(command.cmd)}`);
    }
  }, "ws-archon");
  const manager = new ConnectionManager({ store, host: { kind: "browser", storage: { get: async () => undefined, set: async () => {} } }, clientVersion: "test", random: () => 0 });
  manager.add({ daemon: "archon", transport: daemon.transport, target: "wss://host/archon/ws", label: "host" });
  manager.start();
  return { store, daemon, manager, conn: () => store.get().archon[CONN] };
}

const settle = () => vi.advanceTimersByTimeAsync(0);

beforeEach(() => { vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] }); });
afterEach(() => { vi.useRealTimers(); });

describe("stamped reads", () => {
  it("replace an entry only when at least as new, and keep a bounded record to its newest", () => {
    const held = withStamped({}, "a", { value: { v: 1 }, asOf: 10 });
    expect(withStamped(held, "a", { value: { v: 0 }, asOf: 9 })).toBe(held);
    expect(withStamped(held, "a", { value: { v: 2 }, asOf: 10 }).a?.value).toEqual({ v: 2 });
    let record = {};
    for (let n = 0; n < 4; n++) record = withStamped(record, `k${n}`, { value: {}, asOf: n }, 3);
    expect(Object.keys(record)).toEqual(["k1", "k2", "k3"]);
  });

  it("key a search by its fields whatever their order, and refuse a range that ends first", () => {
    expect(searchKey({ text: "sync", updated_since: "2026-11-01T00:00:00Z" })).toBe(searchKey({ updated_since: "2026-11-01T00:00:00Z", text: "sync" }));
    expect(searchKey({ text: "sync" })).not.toBe(searchKey({ text: "sync", limit: 5 }));
    expect(() => checkSearch({ updated_since: "2026-11-02T00:00:00Z", before: "2026-11-01T00:00:00Z" })).toThrow(/starts before it ends/);
    expect(() => checkSearch({ text: "" })).toThrow(/1 to 200/);
  });
});

describe("Archon on-demand reads", () => {
  it("asks for the task graph at its 2,000-node limit", async () => {
    const { daemon, conn } = setup();
    await settle();
    expect(daemon.commands("task_graph")[0]).toMatchObject({ limit: GRAPH_LIMIT });
    expect(conn()?.graph?.asOf).toBe(100);
  });

  it("keeps active items visible when old completed items fill the graph limit", async () => {
    const history = Array.from({ length: GRAPH_LIMIT }, (_, index) => ({ id: `old_${index}`, state: "succeeded" }));
    const { daemon, conn } = setup({ task_graph: (command) => {
      const filter = command.filter as Json;
      const active = filter.include_terminal_since === undefined;
      return { ok: { reply: "task_graph", data: { nodes: active ? [{ id: "active", state: "running" }] : history,
        edges: [], initiatives: [], as_of: active ? 101 : 100, truncated: !active } } };
    } });
    await settle();
    expect(daemon.commands("task_graph")).toHaveLength(2);
    expect(conn()?.graph?.nodes.active).toBeDefined();
    expect(Object.keys(conn()?.graph?.nodes ?? {})).toHaveLength(GRAPH_LIMIT);
    expect(conn()?.graph?.truncated).toBe(true);
    expect(conn()?.graph?.asOf).toBe(100);
  });

  it("stamps an item history with the cursor, and reads it again when a change overtook it", async () => {
    let reads = 0;
    const { daemon, manager, conn } = setup({
      item_history: (_command, self) => {
        reads += 1;
        if (reads === 1) self.push(change(121, "qi_1"));
        return { ok: reads === 1 ? transitions(1) : transitions(1, 121) };
      },
    });
    await settle();
    await manager.readHistory(CONN, "qi_1");
    expect(daemon.commands("item_history")).toHaveLength(2);
    expect(daemon.commands("item_history")[0]).toMatchObject({ item: "qi_1", limit: HISTORY_LIMIT });
    const stored = conn()?.histories.qi_1;
    expect((stored?.value.transitions as Json[]).map((row) => row.seq)).toEqual([1, 121]);
    expect(stored?.asOf).toBe(conn()?.changeCursor);
  });

  it("refuses a history out of seq order, and keeps nothing for it", async () => {
    const { manager, conn } = setup({ item_history: () => ({ ok: transitions(5, 3) }) });
    await settle();
    await expect(manager.readHistory(CONN, "qi_1")).rejects.toThrow(/ascending seq/);
    expect(conn()?.histories.qi_1).toBeUndefined();
  });

  it("reads an initiative into the store and reports one archond does not have", async () => {
    const detail = { initiative: { id: "in_1", title: "One", state: "open", contract: "", created_at: "x", updated_at: "x" }, members: [] };
    const { manager, conn } = setup({
      get_initiative: (command) => command.initiative === "in_1" ? { ok: { reply: "initiative", data: detail } } : { err: { code: "not_found", message: "no initiative" } },
    });
    await settle();
    await manager.readInitiative(CONN, "in_1");
    expect(conn()?.initiatives.in_1).toEqual({ value: detail, asOf: conn()?.changeCursor });
    await expect(manager.readInitiative(CONN, "in_2")).rejects.toMatchObject({ code: "not_found" });
    expect(Object.keys(conn()?.initiatives ?? {})).toEqual(["in_1"]);
  });

  it("stores each search under its key and keeps only the newest few", async () => {
    const { daemon, manager, conn } = setup({ list_items: (command) => ({ ok: { reply: "items", data: [{ id: `qi_${String(command.text)}` }] } }) });
    await settle();
    for (let n = 0; n <= SEARCHES_KEPT; n++) await manager.searchItems(CONN, { text: `t${n}`, limit: 5 });
    expect(daemon.commands("list_items")[0]).toEqual(expect.objectContaining({ text: "t0", limit: 5 }));
    expect(Object.keys(conn()?.searches ?? {})).toHaveLength(SEARCHES_KEPT);
    expect(conn()?.searches[searchKey({ text: "t0", limit: 5 })]).toBeUndefined();
    expect(conn()?.searches[searchKey({ text: `t${SEARCHES_KEPT}`, limit: 5 })]?.value).toEqual({ items: [{ id: `qi_t${SEARCHES_KEPT}` }] });
  });

  it("does not send filters or history reads to an archond without them", async () => {
    const { daemon, manager } = setup({}, F.welcome_24);
    await settle();
    await expect(manager.searchItems(CONN, { text: "x" })).rejects.toThrow(/item_filters|filters/);
    await expect(manager.readHistory(CONN, "qi_1")).rejects.toThrow(/item_history/);
    expect(daemon.commands("list_items")).toHaveLength(0);
  });
});
