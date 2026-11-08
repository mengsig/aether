// What a Noesis change's reply does to the store: an answer or a message makes its session stale, so
// the screen rereads it and shows the recorded grade without waiting for an event.

import { describe, expect, test } from "vitest";
import { installNoesisChange } from "./noesis-reads.ts";
import { EMPTY_AETHER } from "./store.ts";

const OK = { reply: "ok", data: null };

describe("installNoesisChange", () => {
  test("an answer, message and retry mark their session stale", () => {
    const answered = installNoesisChange(EMPTY_AETHER, { cmd: "noesis_answer", session: "ns_1", question: "q_1", answer: { dont_know: true } }, OK, 1);
    expect(answered.learn.versions).toEqual({ "session:ns_1": 1 });
    const sent = installNoesisChange(answered, { cmd: "noesis_send", session: "ns_1", text: "Why?" }, OK, 2);
    expect(sent.learn.versions).toEqual({ "session:ns_1": 2 });
    const retried = installNoesisChange(sent, { cmd: "noesis_retry_turn", session: "ns_1" }, OK, 3);
    expect(retried.learn.versions).toEqual({ "session:ns_1": 3 });
  });

  test("an extraction marks the job list stale; other changes leave the store as it was", () => {
    expect(installNoesisChange(EMPTY_AETHER, { cmd: "noesis_extract", session: "ns_1" }, OK, 1).learn.versions).toEqual({ jobs: 1 });
    expect(installNoesisChange(EMPTY_AETHER, { cmd: "noesis_import_source", text: "notes" }, OK, 1)).toBe(EMPTY_AETHER);
  });

  test("an answer that names no session is a programming error", () => {
    expect(() => installNoesisChange(EMPTY_AETHER, { cmd: "noesis_answer" }, OK, 1)).toThrow();
  });
});
