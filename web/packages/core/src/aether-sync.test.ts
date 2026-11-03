import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Json } from "./guards.ts";
import { ConnectionManager } from "./manager.ts";
import { INITIAL_STATE, Store } from "./store.ts";
import { FakeDaemon, type Handler } from "./fixtures/fake-daemon.ts";
import F from "./fixtures/aether-frames.json";

const CONN_A = "aether:inst-a";
const CONN_B = "aether:inst-b";

function setup(handler: Handler) {
  const store = new Store(INITIAL_STATE, (flush) => flush());
  const daemon = new FakeDaemon(handler);
  const manager = new ConnectionManager({ store, host: { kind: "browser", storage: { get: async () => undefined, set: async () => {} } }, clientVersion: "test", random: () => 0 });
  manager.add({ daemon: "aether", transport: daemon.transport, target: "wss://host/ws", label: "host" });
  return { store, daemon, manager };
}

/** Answers like a healthy aetherd: a fresh subscribe replays the first events, a resumed one the replay. */
function healthy(overrides: Partial<Record<string, (command: Json) => ReturnType<Handler>>> = {}): Handler {
  return (command, daemon) => {
    const override = overrides[String(command.cmd)];
    if (override) return override(command);
    switch (command.cmd) {
      case "hello": return { ok: F.welcome_a };
      case "subscribe":
        if (command.deltas) return { ok: F.subscribed_deltas };
        return { ok: F.subscribed_durable, then: command.after === null ? F.events_first : F.events_replay };
      case "get_attention": return { ok: F.attention };
      case "get_tree": return { ok: F.tree };
      case "get_agent": return { ok: { reply: "agent", data: F.tree.data.nodes[F.agent as keyof typeof F.tree.data.nodes].agent } };
      case "get_transcript_page": return { ok: F.transcript_page };
      case "unsubscribe": return { ok: { reply: "ok", data: null } };
      default: throw new Error(`unscripted ${String(command.cmd)} (${daemon.sent.length} sent)`);
    }
  };
}

const settle = () => vi.advanceTimersByTimeAsync(0);

beforeEach(() => { vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] }); });
afterEach(() => { vi.useRealTimers(); });

describe("Aether resync (CONTRACTS.md §5.2)", () => {
  it("reads snapshots on first ready and keeps only events newer than the snapshot", async () => {
    const { store, daemon, manager } = setup(healthy());
    manager.start();
    await settle();
    const state = store.get().aether[CONN_A];
    expect(store.get().connections[CONN_A]?.state).toBe("ready");
    expect(daemon.commands("subscribe")[0]).toMatchObject({ after: null, query: {}, deltas: false });
    expect(Object.keys(state?.agents ?? {})).toEqual([F.agent, F.child]);
    // The attention snapshot was taken at seq 2: events 1 and 2 are in it already.
    expect(state?.events.map((e) => e.seq)).toEqual([3]);
    expect(state?.lastSeq).toBe(3);
  });

  it("replays after a reconnect from last_seq without rereading snapshots", async () => {
    const { store, daemon, manager } = setup(healthy());
    manager.start();
    await settle();
    daemon.drop(1006);
    expect(store.get().connections[CONN_A]?.state).toBe("backoff");
    await vi.advanceTimersByTimeAsync(300);
    expect(daemon.opens).toBe(2);
    expect(daemon.commands("subscribe")[1]).toMatchObject({ after: 3, deltas: false });
    expect(daemon.commands("get_attention")).toHaveLength(1);
    expect(store.get().aether[CONN_A]?.events.map((e) => e.seq)).toEqual([3, 4, 5]);
    expect(store.get().aether[CONN_A]?.lastSeq).toBe(5);
  });

  it("drops an event whose seq is at or below last_seq", async () => {
    const { store, daemon, manager } = setup(healthy());
    manager.start();
    await settle();
    daemon.drop(1006);
    await vi.advanceTimersByTimeAsync(300);
    const before = store.get().aether[CONN_A];
    daemon.push(F.event_stale);
    expect(store.get().aether[CONN_A]).toBe(before);
    daemon.push(F.event_next);
    expect(store.get().aether[CONN_A]?.lastSeq).toBe(6);
    expect(store.get().aether[CONN_A]?.events.filter((e) => e.seq === 4)).toHaveLength(1);
  });

  it("coalesces agent refetches after lifecycle events into one get_agent", async () => {
    const { daemon, manager } = setup(healthy());
    manager.start();
    await settle();
    daemon.push(F.event_next);
    daemon.push({ ...F.event_next, event: { ...F.event_next.event, seq: 7 } });
    await vi.advanceTimersByTimeAsync(250);
    expect(daemon.commands("get_agent")).toHaveLength(1);
  });

  it("rereads the transcript on an invocation_seq gap and discards the stream", async () => {
    const { store, daemon, manager } = setup(healthy());
    manager.start();
    await settle();
    await manager.watchAgents(CONN_A, [F.agent]);
    await settle();
    expect(daemon.commands("subscribe").at(-1)).toMatchObject({ after: null, query: { agent: F.agent }, deltas: true });
    expect(daemon.commands("get_transcript_page")).toHaveLength(1);
    const [first, second, gap] = F.deltas_gap;
    daemon.push(first as Json);
    daemon.push(second as Json);
    expect(store.get().aether[CONN_A]?.streams[F.invocation]?.text).toBe("Hello");
    daemon.push(gap as Json);
    await settle();
    expect(store.get().aether[CONN_A]?.streams[F.invocation]?.text).toBe("");
    expect(daemon.commands("get_transcript_page")).toHaveLength(2);
    expect(daemon.commands("get_transcript_page")[1]).toMatchObject({ agent: F.agent, after: 5 });
  });

  it("reads the transcript first when the first delta of an invocation is past 1", async () => {
    const { store, daemon, manager } = setup(healthy());
    manager.start();
    await settle();
    await manager.watchAgents(CONN_A, [F.agent]);
    await settle();
    daemon.push({ ...F.delta_midway, invocation: "inv_other" });
    await settle();
    expect(daemon.commands("get_transcript_page")).toHaveLength(2);
    expect(store.get().aether[CONN_A]?.streams.inv_other?.text).toBe("");
    daemon.push({ ...F.delta_midway, invocation: "inv_other", invocation_seq: 8, delta: { delta: "text", text: "later" } });
    expect(store.get().aether[CONN_A]?.streams.inv_other?.text).toBe("later");
  });

  it("clears the cache when the instance_id changes and moves unconfirmed submissions to review", async () => {
    let welcome = F.welcome_a;
    const { store, daemon, manager } = setup(healthy({ hello: () => ({ ok: welcome }), send_message: () => undefined }));
    manager.start();
    await settle();
    const sending = manager.send(CONN_A, { cmd: "send_message", to: F.agent, body: "hi" });
    expect(store.get().aether[CONN_A]?.outbox[0]?.state).toBe("pending");
    welcome = F.welcome_b;
    daemon.drop(1006);
    await expect(sending).rejects.toMatchObject({ code: "disconnected" });
    await vi.advanceTimersByTimeAsync(300);
    const state = store.get();
    expect(Object.keys(state.connections)).toEqual([CONN_B]);
    expect(state.aether[CONN_A]).toBeUndefined();
    expect(state.review).toHaveLength(1);
    expect(state.review[0]).toMatchObject({ conn: CONN_A, state: "unknown", command: { cmd: "send_message" } });
    expect(daemon.commands("subscribe")[1]).toMatchObject({ after: null });
    expect(daemon.commands("get_attention")).toHaveLength(2);
  });

  it("runs a full resync when the cursor's history expired", async () => {
    let attention = F.attention;
    const { store, daemon, manager } = setup(healthy({
      subscribe: (command) => {
        if (command.after === 3) return { err: F.history_expired };
        return { ok: F.subscribed_durable, then: command.after === null && daemon.opens > 1 ? F.events_replay : F.events_first };
      },
      get_attention: () => ({ ok: attention }),
    }));
    manager.start();
    await settle();
    attention = { ...F.attention, data: { ...F.attention.data, last_seq: 4 } };
    daemon.drop(1006);
    await vi.advanceTimersByTimeAsync(300);
    expect(daemon.commands("subscribe").map((c) => c.after)).toEqual([null, 3, null]);
    expect(daemon.commands("get_attention")).toHaveLength(2);
    expect(daemon.commands("get_tree")).toHaveLength(2);
    // Buffered live events apply after the snapshot, newer than its watermark only.
    expect(store.get().aether[CONN_A]?.events.map((e) => e.seq)).toEqual([3, 5]);
    expect(store.get().aether[CONN_A]?.lastSeq).toBe(5);
  });

  it("discards in-flight streams on a lag close and rereads watched transcripts", async () => {
    const { store, daemon, manager } = setup(healthy());
    manager.start();
    await settle();
    await manager.watchAgents(CONN_A, [F.agent]);
    await settle();
    daemon.push(F.deltas_gap[0] as Json);
    daemon.drop(1013, "lagged");
    expect(store.get().aether[CONN_A]?.streams).toEqual({});
    await vi.advanceTimersByTimeAsync(300);
    expect(daemon.commands("subscribe").filter((c) => c.deltas === true)).toHaveLength(2);
    expect(daemon.commands("get_transcript_page").length).toBeGreaterThanOrEqual(2);
  });
});
