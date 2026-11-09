// @vitest-environment happy-dom
import { afterEach, expect, test, vi } from "vitest";
import type { aether } from "@aether-ui/protocol";
import { ConnectionManager } from "./manager.ts";
import { INITIAL_STATE, Store } from "./store.ts";
import { FakeDaemon } from "./fixtures/fake-daemon.ts";
import { EMPTY_NOESIS, installAllSessions, installSession, nextTicket, removeSession, sessionMembership } from "./noesis-cache.ts";
import { noesisReadCommand } from "./noesis-reads.ts";

const CONN = "aether:@test";
function connected() {
  const store = new Store(INITIAL_STATE, (flush) => flush());
  const manager = new ConnectionManager({ store, host: { kind: "browser", storage: { get: async () => undefined, set: async () => {} } }, clientVersion: "test" });
  const daemon = new FakeDaemon(() => { throw new Error("unexpected request"); });
  manager.add({ daemon: "aether", transport: daemon.transport, target: "test", label: "test" });
  return { app: { manager, store }, settle: async () => {}, unmount: () => manager.stop() };
}
let h: ReturnType<typeof connected> | undefined;
afterEach(() => { h?.unmount(); h = undefined; });
const row = (n: number): aether.NoesisSession => ({ id: `ns_${n}`, agent: "agt_1", goal: "Goal", mode: "teach", phase: "teach", created_at: "2026-11-04T08:00:00Z", updated_at: "2026-11-04T08:00:00Z" });
const reply = (sessions: unknown[], next_after?: unknown) => ({ reply: "noesis_sessions", data: { sessions, next_after } });

test("fetches every page serially and preserves the legacy command", async () => {
  h = connected();
  const send = vi.spyOn(h.app.manager, "send").mockResolvedValueOnce(reply(Array.from({ length: 200 }, (_, n) => row(n)), "next"))
    .mockResolvedValueOnce(reply([row(200)]));
  await h.app.manager.readNoesis(CONN, { kind: "all-sessions" });
  await h.settle();
  expect(send.mock.calls.map((call) => call[1])).toEqual([{ cmd: "noesis_list_sessions", limit: 200 }, { cmd: "noesis_list_sessions", limit: 200, after: "next" }]);
  expect(h.app.store.get().aether[CONN]?.learn.reads["all-sessions"]?.value.ids).toHaveLength(201);
  expect(noesisReadCommand({ kind: "sessions", limit: 20 })).toEqual({ cmd: "noesis_list_sessions", limit: 20 });
});

test.each(["repeat", "malformed", "row", "duplicate", "failure"])("rejects %s on a later page without installing partial results", async (kind) => {
  h = connected();
  const send = vi.spyOn(h.app.manager, "send").mockResolvedValueOnce(reply([row(1)], "next"));
  if (kind === "failure") send.mockRejectedValueOnce(new Error("offline"));
  else if (kind === "repeat") send.mockResolvedValueOnce(reply([row(2)], "next"));
  else if (kind === "row") send.mockResolvedValueOnce(reply([{ id: "bad" }]));
  else if (kind === "duplicate") send.mockResolvedValueOnce(reply([row(1)]));
  else send.mockResolvedValueOnce(reply([row(2)], 42));
  await expect(h.app.manager.readNoesis(CONN, { kind: "all-sessions" })).rejects.toThrow();
  await h.settle();
  expect(h.app.store.get().aether[CONN]?.learn.reads["all-sessions"]).toBeUndefined();
  expect(h.app.store.get().aether[CONN]?.learn.sessions.ns_1).toBeUndefined();
});

test("older snapshots cannot undo newer reads, entity updates, creations or deletions", () => {
  let cache = nextTicket(nextTicket(EMPTY_NOESIS));
  cache = installAllSessions(cache, { sessions: [row(1)] }, 2);
  expect(installAllSessions(cache, { sessions: [row(2)] }, 1)).toBe(cache);
  cache = nextTicket(cache);
  const ticket = cache.ticket;
  cache = installSession(cache, { ...row(2), goal: "new", updated_at: "2099-01-01T00:00:00Z" });
  cache = sessionMembership(installSession(cache, row(3)), "ns_3", true);
  cache = removeSession(cache, "ns_1");
  cache = installAllSessions(cache, { sessions: [row(1), row(2)] }, ticket);
  expect(cache.reads["all-sessions"]?.value.ids).toEqual(["ns_2", "ns_3"]);
  expect(cache.sessions.ns_1).toBeUndefined();
  expect(cache.sessions.ns_2?.goal).toBe("new");
  expect(cache.sessions.ns_3).toBeDefined();
});

test("overlapping reads cannot replace a newer completed snapshot", async () => {
  h = connected();
  let finish: ((value: ReturnType<typeof reply>) => void) | undefined;
  const slow = new Promise<ReturnType<typeof reply>>((resolve) => { finish = resolve; });
  vi.spyOn(h.app.manager, "send").mockReturnValueOnce(slow).mockResolvedValueOnce(reply([row(2)]));
  const older = h.app.manager.readNoesis(CONN, { kind: "all-sessions" });
  await h.app.manager.readNoesis(CONN, { kind: "all-sessions" });
  finish?.(reply([row(1)]));
  await older;
  await h.settle();
  expect(h.app.store.get().aether[CONN]?.learn.reads["all-sessions"]?.value.ids).toEqual(["ns_2"]);
});

test("a failed refresh keeps the previous completed snapshot", async () => {
  h = connected();
  const send = vi.spyOn(h.app.manager, "send").mockResolvedValueOnce(reply([row(1)]));
  await h.app.manager.readNoesis(CONN, { kind: "all-sessions" });
  send.mockResolvedValueOnce(reply([row(2)], "next")).mockRejectedValueOnce(new Error("offline"));
  await expect(h.app.manager.readNoesis(CONN, { kind: "all-sessions" })).rejects.toThrow("offline");
  await h.settle();
  expect(h.app.store.get().aether[CONN]?.learn.reads["all-sessions"]?.value.ids).toEqual(["ns_1"]);
  expect(h.app.store.get().aether[CONN]?.learn.sessions.ns_2).toBeUndefined();
});
