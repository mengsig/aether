import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Json } from "./guards.ts";
import { applyEvent } from "./aether-reduce.ts";
import { compareStamps } from "./aether-sync.ts";
import { ConnectionManager } from "./manager.ts";
import { EMPTY_AETHER, INITIAL_STATE, Store } from "./store.ts";
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
      case "get_transcript_page": {
        const entries = F.transcript_page.data.entries.filter((entry) => entry.seq > Number(command.after));
        return { ok: { ...F.transcript_page, data: { ...F.transcript_page.data, entries } } };
      }
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

describe("Aether resync review regressions", () => {
  const agentEvent = (n: number, type: string, extra: Json = {}) => ({ frame: "event", sub: 1, event: { seq: n, id: `evt_${n}`, at: "2026-11-04T03:01:00Z", actor: { actor: "system" }, kind: { type, ...extra }, agent_id: F.agent } });

  it("refetches an agent made stale just before a reconnect", async () => {
    const { daemon, manager } = setup(healthy());
    manager.start();
    await settle();
    daemon.push(agentEvent(4, "agent.completed"));
    daemon.drop(1006);
    await vi.advanceTimersByTimeAsync(300);
    expect(daemon.commands("get_agent")).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(250);
    expect(daemon.commands("get_agent")).toEqual([expect.objectContaining({ id: F.agent })]);
    expect(daemon.commands("get_attention")).toHaveLength(2);
  });

  it("keeps a newer agent row when a refetch answers with an older one", async () => {
    const agent = F.tree.data.nodes[F.agent as keyof typeof F.tree.data.nodes].agent;
    const older = { ...agent, name: "older", updated_at: "2026-11-04T02:59:00Z" };
    const { store, daemon, manager } = setup(healthy({ get_agent: () => ({ ok: { reply: "agent", data: older } }) }));
    manager.start();
    await settle();
    daemon.push(agentEvent(4, "agent.usage_updated", { total: {}, turn_usage: {} }));
    await vi.advanceTimersByTimeAsync(250);
    expect(daemon.commands("get_agent")).toHaveLength(1);
    expect(store.get().aether[CONN_A]?.agents[F.agent]?.name).toBe("worker");
  });

  it("refetches the agent after a usage update", async () => {
    const { daemon, manager } = setup(healthy());
    manager.start();
    await settle();
    daemon.push(agentEvent(4, "agent.usage_updated", { total: {}, turn_usage: {} }));
    await vi.advanceTimersByTimeAsync(250);
    expect(daemon.commands("get_agent")).toHaveLength(1);
  });

  it("keeps the read cursor when live messages arrive, so a reread fills the gap", async () => {
    const { store, daemon, manager } = setup(healthy());
    manager.start();
    await settle();
    await manager.watchAgents(CONN_A, [F.agent]);
    await settle();
    daemon.push(agentEvent(50, "agent.message", { role: "assistant", content: [], invocation: "inv_live" }));
    const transcript = store.get().aether[CONN_A]?.transcripts[F.agent];
    expect(transcript?.entries.map((e) => e.seq)).toEqual([5, 50]);
    expect(transcript?.lastEntrySeq).toBe(5);
    daemon.push(F.deltas_gap[2] as Json);
    await settle();
    expect(daemon.commands("get_transcript_page").at(-1)).toMatchObject({ after: 5 });
  });

  it("drops deltas until the transcript reread they wait for has answered", async () => {
    const { store, daemon, manager } = setup(healthy());
    manager.start();
    await settle();
    await manager.watchAgents(CONN_A, [F.agent]);
    await settle();
    daemon.handler = healthy({ get_transcript_page: () => undefined });
    daemon.push(F.delta_midway);
    daemon.push({ ...F.delta_midway, invocation_seq: 8, delta: { delta: "text", text: "dropped" } });
    expect(store.get().aether[CONN_A]?.streams[F.invocation]).toMatchObject({ text: "", recovering: true, lastInvocationSeq: 8 });
    const read = daemon.commands("get_transcript_page").at(-1);
    daemon.push({ frame: "reply", id: read?.req, result: { Ok: { reply: "transcript_page", data: { entries: [], has_before: false, has_after: false } } } });
    await settle();
    daemon.push({ ...F.delta_midway, invocation_seq: 9, delta: { delta: "text", text: "kept" } });
    expect(store.get().aether[CONN_A]?.streams[F.invocation]).toMatchObject({ text: "kept", recovering: false });
  });

  it("serializes overlapping watch calls into one subscription per watched agent", async () => {
    const { daemon, manager } = setup(healthy());
    manager.start();
    await settle();
    await Promise.all([manager.watchAgents(CONN_A, [F.agent]), manager.watchAgents(CONN_A, [F.agent])]);
    await settle();
    expect(daemon.commands("subscribe").filter((c) => c.deltas === true)).toHaveLength(1);
    const switching = manager.watchAgents(CONN_A, [F.child]);
    await Promise.all([switching, manager.watchAgents(CONN_A, [F.child])]);
    await settle();
    const deltaSubs = daemon.commands("subscribe").filter((c) => c.deltas === true);
    expect(deltaSubs.map((c) => (c.query as Json).agent)).toEqual([F.agent, F.child]);
    expect(daemon.commands("unsubscribe")).toEqual([expect.objectContaining({ id: 2 })]);
  });

  it("keeps a keyed command unknown when the link is revoked before its answer", async () => {
    const { store, daemon, manager } = setup(healthy({ send_message: () => undefined }));
    manager.start();
    await settle();
    const sending = manager.send(CONN_A, { cmd: "send_message", to: F.agent, body: "hi" });
    daemon.drop(4401, "revoked");
    await expect(sending).rejects.toMatchObject({ code: "disconnected" });
    expect(store.get().aether[CONN_A]?.outbox).toEqual([expect.objectContaining({ state: "unknown" })]);
    expect(store.get().connections[CONN_A]?.state).toBe("pairing");
  });
});

describe("submission horizon", () => {
  const withHorizon = (horizon: string) => ({ ...F.welcome_a, data: { ...F.welcome_a.data, submission_horizon: horizon } });

  it("records the welcome's horizon on a lost keyed command and keeps the first one across a resend", async () => {
    let welcome = withHorizon("hz_first");
    const { store, daemon, manager } = setup(healthy({ hello: () => ({ ok: welcome }), send_message: () => undefined }));
    manager.start();
    await settle();
    const first = manager.send(CONN_A, { cmd: "send_message", to: F.agent, body: "hi" });
    welcome = withHorizon("hz_later");
    daemon.drop(1006);
    await expect(first).rejects.toMatchObject({ code: "disconnected" });
    await vi.advanceTimersByTimeAsync(300);
    const lost = store.get().aether[CONN_A]?.outbox[0];
    expect(lost).toMatchObject({ state: "unknown", horizon: "hz_first" });
    const again = manager.send(CONN_A, lost?.command ?? {});
    expect(store.get().aether[CONN_A]?.outbox[0]).toMatchObject({ key: lost?.key, horizon: "hz_first" });
    daemon.drop(1006);
    await expect(again).rejects.toMatchObject({ code: "disconnected" });
  });

  it("records no horizon when the daemon names none", async () => {
    const { store, manager, daemon } = setup(healthy({ send_message: () => undefined }));
    manager.start();
    await settle();
    const sending = manager.send(CONN_A, { cmd: "send_message", to: F.agent, body: "hi" });
    expect(store.get().aether[CONN_A]?.outbox[0]).not.toHaveProperty("horizon");
    daemon.drop(1006);
    await expect(sending).rejects.toMatchObject({ code: "disconnected" });
  });
});

describe("Aether resync, second review", () => {
  const event = (n: number, type: string, agent: string, extra: Json = {}) => ({ frame: "event", sub: 1, event: { seq: n, id: `evt_${n}`, at: "2026-11-04T03:02:00Z", actor: { actor: "system" }, kind: { type, ...extra }, agent_id: agent } });

  it("does not fail the connection twice when a ping dies with its link", async () => {
    const idle = { ...F.welcome_a, data: { ...F.welcome_a.data, idle_timeout_secs: 1 } };
    const { store, daemon, manager } = setup(healthy({ hello: () => ({ ok: idle }), ping: () => undefined }));
    manager.start();
    await settle();
    await vi.advanceTimersByTimeAsync(500);
    expect(daemon.commands("ping")).toHaveLength(1);
    daemon.drop(4401, "revoked");
    await vi.advanceTimersByTimeAsync(5_000);
    expect(store.get().connections[CONN_A]?.state).toBe("pairing");
    expect(daemon.opens).toBe(1);
  });

  it("adds a live-created root agent to the root index", async () => {
    const created = { ...F.tree.data.nodes[F.agent as keyof typeof F.tree.data.nodes].agent, id: "agt_new" };
    const { store, daemon, manager } = setup(healthy({ get_agent: () => ({ ok: { reply: "agent", data: created } }) }));
    manager.start();
    await settle();
    daemon.push(event(4, "agent.created", "agt_new"));
    await vi.advanceTimersByTimeAsync(250);
    expect(store.get().aether[CONN_A]?.roots).toEqual([F.agent, "agt_new"]);
    expect(store.get().aether[CONN_A]?.agents.agt_new).toEqual(created);
  });

  it("sends a second refresh of one agent only after the first has answered", async () => {
    const { daemon, manager } = setup(healthy({ get_agent: () => undefined }));
    manager.start();
    await settle();
    daemon.push(event(4, "agent.started", F.agent));
    await vi.advanceTimersByTimeAsync(250);
    daemon.push(event(5, "agent.completed", F.agent));
    await vi.advanceTimersByTimeAsync(250);
    expect(daemon.commands("get_agent")).toHaveLength(1);
    const first = daemon.commands("get_agent")[0];
    daemon.push({ frame: "reply", id: first?.req, result: { Ok: { reply: "agent", data: F.tree.data.nodes[F.agent as keyof typeof F.tree.data.nodes].agent } } });
    await settle();
    expect(daemon.commands("get_agent")).toHaveLength(2);
  });

  it("derives noesis enabled from the effective settings an event carries", () => {
    const state = { ...EMPTY_AETHER, lastSeq: 1, noesis: { enabled: false, settings: {}, effective: { "noesis.enabled": false } } };
    const next = applyEvent(state, { seq: 2, id: "e2", at: "2026-11-04T03:00:00Z", actor: { actor: "system" }, kind: { type: "noesis.settings_changed", settings: { "noesis.enabled": true }, effective: { "noesis.enabled": true } } } as never);
    expect(next.noesis).toMatchObject({ enabled: true, effective: { "noesis.enabled": true } });
    expect(next.lastSeq).toBe(2);
  });

  it("rereads attention when an answer or archive resolves an entry, but not for streaming", () => {
    const base = { ...EMPTY_AETHER, lastSeq: 1 };
    const at = (n: number, type: string) => ({ seq: n, id: `e${n}`, at: "2026-11-04T03:00:00Z", actor: { actor: "system" }, kind: { type }, agent_id: F.agent }) as never;
    for (const type of ["message.sent", "inbox.delivered", "agent.archived", "agent.restarted"]) {
      expect(applyEvent(base, at(2, type)).staleAttention).toBe(true);
    }
    for (const type of ["agent.message", "agent.usage_updated", "invocation.completed", "noesis.settings_changed"]) {
      expect(applyEvent(base, at(2, type)).staleAttention).toBe(false);
    }
  });

  it("retries an attention refresh that timed out on a live link", async () => {
    const { store, daemon, manager } = setup(healthy({ get_attention: () => (daemon.commands("get_attention").length === 2 ? undefined : { ok: F.attention }) }));
    manager.start();
    await settle();
    daemon.push(event(4, "approval.requested", F.agent));
    await vi.advanceTimersByTimeAsync(250 + 20_000);
    expect(store.get().reports).toEqual([expect.objectContaining({ code: "timeout" })]);
    await vi.advanceTimersByTimeAsync(250);
    expect(daemon.commands("get_attention")).toHaveLength(3);
    expect(store.get().aether[CONN_A]?.staleAttention).toBe(false);
  });
});

describe("timestamp order", () => {
  it("tells apart stamps within one millisecond, across precisions and offsets", () => {
    expect(compareStamps("2026-11-04T03:00:00.123456Z", "2026-11-04T03:00:00.123123Z")).toBeGreaterThan(0);
    expect(compareStamps("2026-11-04T03:00:00.1Z", "2026-11-04T03:00:00.100000000Z")).toBe(0);
    expect(compareStamps("2026-11-04T04:00:00.5+01:00", "2026-11-04T03:00:00.4Z")).toBeGreaterThan(0);
    expect(compareStamps("2026-11-04T03:00:00Z", "2026-11-04T03:00:01Z")).toBeLessThan(0);
  });

  it("refuses a stamp that is not RFC 3339", () => {
    expect(() => compareStamps("yesterday", "2026-11-04T03:00:00Z")).toThrow(/RFC 3339/);
  });
});
