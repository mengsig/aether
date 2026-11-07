import type { StreamState } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { describe, expect, test } from "vitest";
import { entryRows, invocationMeta, isDelivered, liveRows, pairTools } from "./rows.ts";

const AGENT = "agt_1";
const entry = (seq: number, message: aether.Message, invocation: string | null = null): aether.TranscriptEntry => ({ seq, at: "2026-11-04T09:00:00Z", invocation, message });
const stream = (invocation: string, fields: Partial<StreamState> = {}): StreamState => ({
  agent: AGENT, invocation, text: "", thinking: "", tools: [], lastInvocationSeq: 1, recovering: false, partial: false, committed: false, ...fields,
});

describe("transcript rows", () => {
  const entries = [
    entry(1, { role: "assistant", content: [{ type: "thinking", text: "plan" }, { type: "tool_use", id: "t1", name: "bash", input: { cmd: "ls" } }] }, "inv_a"),
    entry(2, { role: "user", content: [{ type: "tool_result", tool_use_id: "t1", content: [{ type: "text", text: "a.txt" }, { type: "image", data: "", media_type: "image/png" }] }] }),
    entry(3, { role: "user", content: [{ type: "text", text: "see" }, { type: "image_ref", id: "att_1", media_type: "image/png", size_bytes: 2048, name: "shot.png" }] }),
  ];

  test("pair each tool call with its result and drop the message left empty", () => {
    const rows = entryRows(entries, pairTools(entries), new Map());
    expect(rows.map((row) => row.key)).toEqual(["e:1", "e:3"]);
    expect(rows[0]?.parts).toEqual([
      { kind: "reasoning", text: "plan", redacted: false },
      { kind: "tool", id: "t1", name: "bash", input: "{\n  \"cmd\": \"ls\"\n}", result: { text: "a.txt", isError: false, images: 1 } },
    ]);
    expect(rows[1]?.parts[1]).toEqual({ kind: "attachment", name: "shot.png", mediaType: "image/png", sizeBytes: 2048 });
  });

  test("refuse entries out of seq order", () => {
    expect(() => entryRows([entries[1], entries[0]] as aether.TranscriptEntry[], pairTools(entries), new Map())).toThrow(/ascend/);
  });

  test("attach the model and cost of each invocation from its events", () => {
    const usage = { input_tokens: 1, output_tokens: 2, cache_read_tokens: 0, cache_write_tokens: 0, cost_usd: 0.5, cost_complete: true };
    const events = [
      { seq: 9, agent_id: AGENT, kind: { type: "invocation.completed", invocation: "inv_a", model: { provider: "p", model: "m" }, usage, duration_ms: 10, snapshot: "s" } },
      { seq: 10, agent_id: "agt_other", kind: { type: "invocation.failed", invocation: "inv_a", error: "nope", retryable: false, snapshot: "s" } },
    ] as unknown as aether.Event[];
    const meta = invocationMeta(events, AGENT);
    expect(meta.get("inv_a")).toEqual({ model: "p/m", usage, durationMs: 10, error: null });
    expect(entryRows(entries, pairTools(entries), meta)[0]?.meta?.model).toBe("p/m");
  });
});

describe("live tails", () => {
  test("show only uncommitted tails of this agent, oldest invocation first", () => {
    const streams = {
      inv_b: stream("inv_b", { text: "second" }),
      inv_a: stream("inv_a", { thinking: "hmm", text: "first", tools: [{ id: "t9", name: "bash", input: "{" }] }),
      inv_c: stream("inv_c", { text: "tombstone", committed: true }),
      inv_d: stream("inv_d", { text: "already in the transcript" }),
      inv_e: stream("inv_e", { agent: "agt_other", text: "elsewhere" }),
      inv_f: stream("inv_f"),
      inv_g: stream("inv_g", { recovering: true, partial: true }),
    };
    const rows = liveRows(streams, AGENT, new Set(["inv_d"]));
    expect(rows.map((row) => row.key)).toEqual(["s:inv_a", "s:inv_b", "s:inv_g"]);
    expect(rows[0]?.parts.map((part) => part.kind)).toEqual(["reasoning", "text", "tool"]);
    expect(rows[2]).toMatchObject({ parts: [], live: { recovering: true, partial: true } });
  });
});

describe("a queued message is delivered", () => {
  const said = (seq: number, text: string) => entry(seq, { role: "user", content: [{ type: "text", text }] });
  const rowsOf = (...entries: aether.TranscriptEntry[]) => entryRows(entries, pairTools(entries), new Map());
  const queued = { text: "Ship it.", at: "2026-11-04T09:01:00Z", afterSeq: 2 };

  test("when a later user row carries it under the operator's own header", () => {
    expect(isDelivered(rowsOf(said(3, "[inbox 1] Message from user:device:\nShip it.")), queued)).toBe(true);
    expect(isDelivered(rowsOf(said(3, "[inbox 1] Message from user: [authenticated operator instruction]\nShip it.")), queued)).toBe(true);
  });

  test("when the operator's item follows an agent's or a notice in one inbox batch", () => {
    const afterAgent = "[inbox 1] Message from agt_worker:\nChecks passed.\n[inbox 2] Message from user:local_socket: [authenticated operator instruction]\nShip it.\n";
    const afterNotice = "[inbox 1] Task tsk_1 completed\n[inbox 2] Message from user:device:\nShip it.";
    expect(isDelivered(rowsOf(said(3, afterAgent)), queued)).toBe(true);
    expect(isDelivered(rowsOf(said(3, afterNotice)), queued)).toBe(true);
  });

  test("not when only an agent, a notice or an older row says the words", () => {
    expect(isDelivered(rowsOf(said(3, "[inbox 1] Message from agt_worker:\nShip it.")), queued)).toBe(false);
    expect(isDelivered(rowsOf(said(3, "[inbox 1] Message from agt_worker:\nShip it.\n[inbox 2] Message from user:device:\nWait.")), queued)).toBe(false);
    expect(isDelivered(rowsOf(said(3, "[inbox 1] Task tsk_1 completed: Ship it.")), queued)).toBe(false);
    expect(isDelivered(rowsOf(said(2, "[inbox 1] Message from user:device:\nShip it.")), queued)).toBe(false);
  });
});
