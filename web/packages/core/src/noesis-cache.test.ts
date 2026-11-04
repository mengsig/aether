// Rereads land whole and in order: an older reply never overwrites a newer one, and a deleted
// session never comes back. Events record what they reveal and mark what they make stale.

import type { aether } from "@aether-ui/protocol";
import { describe, expect, test } from "vitest";
import { applyNoesisEvent, EMPTY_NOESIS, installRead, installSession, installSessionList, nextTicket, noesisKey, removeSession, type NoesisCache } from "./noesis-cache.ts";

const session = (updated: string, goal = "Raft elections"): aether.NoesisSession => ({
  id: "ns_1", agent: "agt_1", goal, mode: "teach", phase: "teach", created_at: "2026-11-04T08:00:00Z", updated_at: updated,
});

function tickets(count: number): NoesisCache {
  let cache = EMPTY_NOESIS;
  for (let i = 0; i < count; i += 1) cache = nextTicket(cache);
  return cache;
}

describe("installRead", () => {
  test("replaces the held read with a later ask's reply, whole", () => {
    let cache = installRead(tickets(2), noesisKey.graph, { nodes: [1], chains: [] }, 1);
    cache = installRead(cache, noesisKey.graph, { nodes: [2] }, 2);
    expect(cache.reads.graph).toEqual({ value: { nodes: [2] }, ticket: 2 });
  });

  test("ignores a reply to an earlier ask that arrives after a later one", () => {
    const later = installRead(tickets(2), noesisKey.learner, { concepts: ["new"] }, 2);
    expect(installRead(later, noesisKey.learner, { concepts: ["old"] }, 1)).toBe(later);
  });

  test("refuses a ticket that was never handed out", () => {
    expect(() => installRead(tickets(1), noesisKey.graph, {}, 2)).toThrow();
  });
});

describe("installSession", () => {
  test("takes a session at least as new and keeps a newer one over an older reply", () => {
    const newer = installSession(EMPTY_NOESIS, session("2026-11-04T09:00:00Z", "newer"));
    expect(installSession(newer, session("2026-11-04T08:59:00Z", "older"))).toBe(newer);
    expect(installSession(newer, session("2026-11-04T09:00:00Z", "same time")).sessions.ns_1?.goal).toBe("same time");
  });

  test("a deleted session stays gone, from a reread or a list", () => {
    const deleted = removeSession(installSession(EMPTY_NOESIS, session("2026-11-04T09:00:00Z")), "ns_1");
    expect(deleted.sessions.ns_1).toBeUndefined();
    expect(installSession(deleted, session("2026-11-04T10:00:00Z")).sessions.ns_1).toBeUndefined();
    const listed = installSessionList(nextTicket(deleted), { sessions: [session("2026-11-04T10:00:00Z")] }, deleted.ticket + 1);
    expect(listed.sessions.ns_1).toBeUndefined();
    expect(listed.versions.sessions).toBe(1);
  });

  test("records the open question so it stays shown once answered", () => {
    const question: aether.Question = { id: "q_1", kind: "recall", multi: false, collapsed: false, concepts: [], posed_at: "2026-11-04T09:00:00Z", prompt: "Why?" };
    const cache = installSession(EMPTY_NOESIS, { ...session("2026-11-04T09:00:00Z"), open_question: question });
    expect(cache.questions.q_1).toEqual({ session: "ns_1", question });
  });
});

describe("applyNoesisEvent", () => {
  const held = installSession(EMPTY_NOESIS, session("2026-11-04T09:00:00Z"));

  test("an answer records its result and marks the session stale", () => {
    const result = { question: "q_1", correct: true, answer_key: ["b"], explanation: "One vote per term." };
    const next = applyNoesisEvent(held, { type: "noesis.answer_recorded", session: "ns_1", result }, "agt_1");
    expect(next.results.q_1).toEqual(result);
    expect(next.versions["session:ns_1"]).toBe(1);
    expect(next.versions.sessions).toBe(1);
  });

  test("a committed message of the session's agent marks only that session stale", () => {
    const next = applyNoesisEvent(held, { type: "agent.message", role: "assistant", content: [] }, "agt_1");
    expect(next.versions).toEqual({ "session:ns_1": 1 });
    expect(applyNoesisEvent(held, { type: "agent.message", role: "assistant", content: [] }, "agt_other")).toBe(held);
  });

  test("learner, graph and settings events mark their reads stale", () => {
    let next = applyNoesisEvent(held, { type: "noesis.learner_updated", concepts: [] }, undefined);
    next = applyNoesisEvent(next, { type: "noesis.graph_merged", job: "j", stats: {} }, undefined);
    next = applyNoesisEvent(next, { type: "noesis.settings_changed" }, undefined);
    expect(next.versions).toMatchObject({ learner: 1, graph: 1, status: 1 });
  });

  test("a settings event outranks a status reply asked before it, not one asked after", () => {
    const asked = nextTicket(held);
    const changed = applyNoesisEvent(asked, { type: "noesis.settings_changed" }, undefined);
    expect(installRead(changed, noesisKey.status, { enabled: false }, asked.ticket)).toBe(changed);
    const later = nextTicket(changed);
    expect(installRead(later, noesisKey.status, { enabled: true }, later.ticket).reads.status?.value).toEqual({ enabled: true });
  });

  test("events of other kinds change nothing", () => {
    expect(applyNoesisEvent(held, { type: "agent.created" }, "agt_1")).toBe(held);
  });
});
