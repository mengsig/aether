// Pure updates of one Aether connection's cached state from durable events,
// deltas and transcript pages (CONTRACTS.md §5.2).

import type { aether } from "@aether-ui/protocol";
import { invariant, object, omit, seq, string, type Json } from "./guards.ts";
import type { AetherConnState, StreamState, TranscriptState } from "./store.ts";

const RECENT_EVENTS = 500;
const MAX_STREAM_CHARS = 2_000_000;
const ATTENTION_TYPES = /^(approval|question)\.|^agent\.(failed|blocked|completed|cancelled|resumed|waiting)$/;

/** Applies a durable event if `seq > last_seq`, marking what it made stale; otherwise returns the state unchanged. */
export function applyEvent(state: AetherConnState, event: aether.Event): AetherConnState {
  const at = seq(event.seq, "event.seq");
  if (state.lastSeq !== null && at <= state.lastSeq) return state;
  const kind = object(event.kind, "event.kind");
  const type = string(kind.type, "event.kind.type");
  let next: AetherConnState = markStale({ ...state, lastSeq: at, events: [...state.events, event].slice(-RECENT_EVENTS) }, type, event.agent_id ?? undefined);
  const agent = event.agent_id ?? undefined;
  const invocation = typeof kind.invocation === "string" ? kind.invocation : undefined;
  if (type === "agent.message" && agent) {
    const entry = { seq: at, at: event.at, invocation: invocation ?? null, message: { role: kind.role, content: kind.content } } as aether.TranscriptEntry;
    if (next.transcripts[agent]) next = { ...next, transcripts: { ...next.transcripts, [agent]: mergeEntries(next.transcripts[agent], [entry], null) } };
    // The committed message replaces the streamed text of its invocation.
    const stream = invocation ? next.streams[invocation] : undefined;
    if (invocation && stream) next = withStream(next, invocation, { ...stream, text: "", thinking: "" });
  }
  if ((type === "invocation.completed" || type === "invocation.failed") && invocation) next = withStream(next, invocation, undefined);
  if (type === "noesis.settings_changed") next = { ...next, noesis: withNoesisSettings(next.noesis, kind) };
  invariant(next.lastSeq === at, "an applied event advances last_seq to its own seq");
  return next;
}

/** Lifecycle and usage events change the agent row; attention events change the attention snapshot. */
function markStale(state: AetherConnState, type: string, agent: string | undefined): AetherConnState {
  const agentStale = agent !== undefined && type.startsWith("agent.") && type !== "agent.message" && !state.staleAgents.includes(agent);
  const attentionStale = ATTENTION_TYPES.test(type) && !state.staleAttention;
  if (!agentStale && !attentionStale) return state;
  return {
    ...state,
    ...(agentStale ? { staleAgents: [...state.staleAgents, agent] } : {}),
    ...(attentionStale ? { staleAttention: true } : {}),
  };
}

/** `noesis_status.enabled` is the effective master switch, so a settings event carries it too. */
function withNoesisSettings(status: Json | null, kind: Json): Json {
  const changed = pick(kind, ["settings", "effective"]);
  const effective = object(changed.effective ?? status?.effective ?? {}, "noesis effective settings");
  const enabled = effective["noesis.enabled"];
  return { ...(status ?? {}), ...changed, ...(typeof enabled === "boolean" ? { enabled } : {}) };
}

function pick(source: Json, keys: readonly string[]): Json {
  return Object.fromEntries(keys.filter((key) => key in source).map((key) => [key, source[key]]));
}

export function withStream(state: AetherConnState, invocation: string, stream: StreamState | undefined): AetherConnState {
  const streams = stream ? { ...state.streams, [invocation]: stream } : omit(state.streams, invocation);
  return { ...state, streams };
}

/**
 * Merges transcript entries by seq. `readThrough` is the seq a transcript read
 * covered; live entries pass null and leave the read cursor alone.
 */
export function mergeEntries(current: TranscriptState | undefined, entries: readonly aether.TranscriptEntry[], readThrough: number | null): TranscriptState {
  const bySeq = new Map((current?.entries ?? []).map((entry) => [entry.seq, entry]));
  for (const entry of entries) bySeq.set(seq(entry.seq, "transcript entry seq"), entry);
  const merged = [...bySeq.values()].sort((a, b) => a.seq - b.seq);
  const lastEntrySeq = Math.max(current?.lastEntrySeq ?? 0, readThrough ?? 0);
  invariant(merged.length >= (current?.entries.length ?? 0), "merging never loses entries");
  return { entries: merged, lastEntrySeq };
}

export type DeltaOutcome = "applied" | "reread" | "ignored";

/**
 * Applies one delta frame to its invocation's stream. A gap, or a first delta
 * past 1, empties the stream and drops deltas until a reread ends recovery.
 */
export function applyDelta(state: AetherConnState, frame: Json): { state: AetherConnState; outcome: DeltaOutcome } {
  const agent = string(frame.agent, "delta.agent");
  const invocation = string(frame.invocation, "delta.invocation");
  const delta = object(frame.delta, "delta.delta");
  const order = frame.invocation_seq === undefined || frame.invocation_seq === null ? null : seq(frame.invocation_seq, "delta.invocation_seq");
  const previous = state.streams[invocation];
  invariant(!previous || previous.agent === agent, "an invocation belongs to one agent");
  const lost: StreamState = { agent, invocation, text: "", thinking: "", lastInvocationSeq: order, recovering: true };
  if (order !== null && previous?.lastInvocationSeq != null && order <= previous.lastInvocationSeq) return { state, outcome: "ignored" };
  if (previous?.recovering) return { state: withStream(state, invocation, { ...previous, lastInvocationSeq: order }), outcome: "ignored" };
  const expected = previous?.lastInvocationSeq == null ? 1 : previous.lastInvocationSeq + 1;
  if (order !== null && order !== expected) return { state: withStream(state, invocation, lost), outcome: "reread" };
  const stream: StreamState = { ...(previous ?? { ...lost, recovering: false }), lastInvocationSeq: order };
  if (delta.delta !== "text" && delta.delta !== "thinking") return { state: withStream(state, invocation, stream), outcome: "applied" };
  const field = delta.delta;
  const text = stream[field] + String(delta.text ?? "");
  if (text.length > MAX_STREAM_CHARS) return { state: withStream(state, invocation, lost), outcome: "reread" };
  return { state: withStream(state, invocation, { ...stream, [field]: text }), outcome: "applied" };
}

/** A transcript reread for this agent succeeded: its streams take later deltas again. */
export function endRecovery(state: AetherConnState, agent: string): AetherConnState {
  const recovering = Object.values(state.streams).filter((stream) => stream.agent === agent && stream.recovering);
  if (recovering.length === 0) return state;
  const streams = { ...state.streams };
  for (const stream of recovering) streams[stream.invocation] = { ...stream, recovering: false };
  return { ...state, streams };
}

/** Drops every in-flight stream: after a reconnect or lag close they cannot be trusted (§5.2.4). */
export function discardStreams(state: AetherConnState): AetherConnState {
  return Object.keys(state.streams).length === 0 ? state : { ...state, streams: {} };
}
