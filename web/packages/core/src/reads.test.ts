import { describe, expect, it } from "vitest";
import { applyRead, failRead, markRequested, readInFlight, readReplaces, settleSubmission, stampOf } from "./reads.ts";
import { dropConnection, EMPTY_AETHER, INITIAL_STATE, type State, type Submission } from "./store.ts";

const CONN = "aether:a";

describe("rereads in the store", () => {
  it("replaces the whole entry with a reread issued later, never with an earlier one answering late", () => {
    let state: State = markRequested(markRequested(INITIAL_STATE, CONN, "memories", 1), CONN, "memories", 2);
    state = applyRead(state, CONN, "memories", 2, { memories: [{ id: "m2" }] });
    state = applyRead(state, CONN, "memories", 1, { memories: [{ id: "m1" }, { id: "old" }] });
    expect(state.reads[CONN]?.memories).toMatchObject({ value: { memories: [{ id: "m2" }] }, issued: 2, requested: 2, error: null });
  });

  it("orders by the daemon's own stamp when both replies carry one", () => {
    let state = applyRead(INITIAL_STATE, CONN, "attention", 1, { last_seq: 9, questions: ["new"] });
    state = applyRead(state, CONN, "attention", 2, { last_seq: 7, questions: ["stale"] });
    expect(state.reads[CONN]?.attention?.value).toEqual({ last_seq: 9, questions: ["new"] });
    state = applyRead(state, CONN, "attention", 3, { last_seq: 9, questions: ["same seq, later read"] });
    expect(state.reads[CONN]?.attention?.value).toEqual({ last_seq: 9, questions: ["same seq, later read"] });
    expect(readReplaces({ asOf: 10, issued: 1 }, state.reads[CONN]?.attention)).toBe(true);
    expect(stampOf({ as_of: 4 })).toBe(4);
    expect(stampOf([1])).toBeNull();
  });

  it("keeps the last good value through a failed reread, and ignores a failure older than the value", () => {
    let state = applyRead(INITIAL_STATE, CONN, "devices", 2, { devices: ["phone"] });
    state = failRead(state, CONN, "devices", 1, "timed out");
    expect(state.reads[CONN]?.devices?.error).toBeNull();
    state = failRead(state, CONN, "devices", 3, "disconnected");
    expect(state.reads[CONN]?.devices).toMatchObject({ value: { devices: ["phone"] }, error: "disconnected" });
  });

  it("never lets an older success clear a newer failure, and settles both", () => {
    let state: State = markRequested(markRequested(INITIAL_STATE, CONN, "memories", 1), CONN, "memories", 2);
    expect(readInFlight(state.reads[CONN]?.memories)).toBe(true);
    state = failRead(state, CONN, "memories", 2, "search failed");
    state = applyRead(state, CONN, "memories", 1, [{ id: "old" }]);
    expect(state.reads[CONN]?.memories).toMatchObject({ error: "search failed", errorIssued: 2, settled: 2, requested: 2 });
    expect(readInFlight(state.reads[CONN]?.memories)).toBe(false);
    state = applyRead(markRequested(state, CONN, "memories", 3), CONN, "memories", 3, [{ id: "new" }]);
    expect(state.reads[CONN]?.memories).toMatchObject({ value: [{ id: "new" }], error: null, errorIssued: 0 });
  });

  it("ignores a failure older than a newer failure", () => {
    let state = failRead(INITIAL_STATE, CONN, "devices", 3, "newest");
    state = failRead(state, CONN, "devices", 2, "older");
    expect(state.reads[CONN]?.devices).toMatchObject({ error: "newest", errorIssued: 3, settled: 3 });
  });

  it("drops a connection's reads with the rest of its cache when its daemon identity changes", () => {
    const state = applyRead(INITIAL_STATE, CONN, "devices", 1, { devices: [] });
    expect(dropConnection(state, CONN).reads[CONN]).toBeUndefined();
  });

  it("settles a submission out of both the outbox and the review", () => {
    const lost: Submission = { conn: CONN, key: "k1", command: { cmd: "cancel" }, state: "unknown" };
    const kept: Submission = { ...lost, key: "k2" };
    const state: State = { ...INITIAL_STATE, review: [lost, kept], aether: { [CONN]: { ...EMPTY_AETHER, outbox: [lost, kept] } } };
    const next = settleSubmission(state, CONN, "k1");
    expect(next.review.map((s) => s.key)).toEqual(["k2"]);
    expect(next.aether[CONN]?.outbox.map((s) => s.key)).toEqual(["k2"]);
  });
});
