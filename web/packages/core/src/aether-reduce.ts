// Pure updates of one Aether connection's cached state from durable events,
// deltas and transcript pages (CONTRACTS.md §5.2).

import type { aether } from "@aether-ui/protocol";
import { invariant, object, omit, seq, string, type Json } from "./guards.ts";
import type { AetherConnState, StreamState, TranscriptState } from "./store.ts";

const RECENT_EVENTS = 500;
const MAX_STREAM_CHARS = 2_000_000;

/** Applies a durable event if `seq > last_seq`; returns the state unchanged otherwise. */
export function applyEvent(state: AetherConnState, event: aether.Event): AetherConnState {
  const at = seq(event.seq, "event.seq");
  if (state.lastSeq !== null && at <= state.lastSeq) return state;
  const kind = object(event.kind, "event.kind");
  const type = string(kind.type, "event.kind.type");
  let next: AetherConnState = { ...state, lastSeq: at, events: [...state.events, event].slice(-RECENT_EVENTS) };
  const agent = event.agent_id ?? undefined;
  const invocation = typeof kind.invocation === "string" ? kind.invocation : undefined;
  if (type === "agent.message" && agent) {
    const entry = { seq: at, at: event.at, invocation: invocation ?? null, message: { role: kind.role, content: kind.content } } as aether.TranscriptEntry;
    if (next.transcripts[agent]) next = { ...next, transcripts: { ...next.transcripts, [agent]: mergeEntries(next.transcripts[agent], [entry]) } };
    // The committed message replaces the streamed text of its invocation.
    if (invocation && next.streams[invocation]) next = withStream(next, invocation, { ...next.streams[invocation], text: "", thinking: "" });
  }
  if ((type === "invocation.completed" || type === "invocation.failed") && invocation) next = withStream(next, invocation, undefined);
  if (type === "noesis.settings_changed") next = { ...next, noesis: { ...(next.noesis ?? {}), ...pick(kind, ["settings", "effective"]) } };
  invariant(next.lastSeq === at, "an applied event advances last_seq to its own seq");
  return next;
}

function pick(source: Json, keys: readonly string[]): Json {
  return Object.fromEntries(keys.filter((key) => key in source).map((key) => [key, source[key]]));
}

export function withStream(state: AetherConnState, invocation: string, stream: StreamState | undefined): AetherConnState {
  const streams = stream ? { ...state.streams, [invocation]: stream } : omit(state.streams, invocation);
  return { ...state, streams };
}

/** Merges transcript entries by seq, keeping them ordered and unique. */
export function mergeEntries(current: TranscriptState | undefined, entries: readonly aether.TranscriptEntry[]): TranscriptState {
  const bySeq = new Map((current?.entries ?? []).map((entry) => [entry.seq, entry]));
  for (const entry of entries) bySeq.set(seq(entry.seq, "transcript entry seq"), entry);
  const merged = [...bySeq.values()].sort((a, b) => a.seq - b.seq);
  const lastEntrySeq = Math.max(current?.lastEntrySeq ?? 0, merged.at(-1)?.seq ?? 0);
  return { entries: merged, lastEntrySeq };
}

export type DeltaOutcome = "applied" | "reread" | "ignored";

/**
 * Applies one delta frame to its invocation's stream. A gap in
 * `invocation_seq`, or a first delta past 1, discards the stream and asks for
 * a transcript reread; only later deltas are applied after it.
 */
export function applyDelta(state: AetherConnState, frame: Json): { state: AetherConnState; outcome: DeltaOutcome } {
  const agent = string(frame.agent, "delta.agent");
  const invocation = string(frame.invocation, "delta.invocation");
  const delta = object(frame.delta, "delta.delta");
  const order = frame.invocation_seq === undefined || frame.invocation_seq === null ? null : seq(frame.invocation_seq, "delta.invocation_seq");
  const previous = state.streams[invocation];
  invariant(!previous || previous.agent === agent, "an invocation belongs to one agent");
  const fresh: StreamState = { agent, invocation, text: "", thinking: "", lastInvocationSeq: order };
  if (order !== null) {
    const expected = previous?.lastInvocationSeq == null ? 1 : previous.lastInvocationSeq + 1;
    if (previous?.lastInvocationSeq != null && order <= previous.lastInvocationSeq) return { state, outcome: "ignored" };
    if (order !== expected) return { state: withStream(state, invocation, fresh), outcome: "reread" };
  }
  const stream = { ...(previous ?? fresh), lastInvocationSeq: order };
  if (delta.delta === "text" || delta.delta === "thinking") {
    const field = delta.delta;
    const text = stream[field] + String(delta.text ?? "");
    if (text.length > MAX_STREAM_CHARS) return { state: withStream(state, invocation, fresh), outcome: "reread" };
    return { state: withStream(state, invocation, { ...stream, [field]: text }), outcome: "applied" };
  }
  return { state: withStream(state, invocation, stream), outcome: "applied" };
}

/** Drops every in-flight stream: after a reconnect or lag close they cannot be trusted (§5.2.4). */
export function discardStreams(state: AetherConnState): AetherConnState {
  return Object.keys(state.streams).length === 0 ? state : { ...state, streams: {} };
}
