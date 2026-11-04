// Pure updates of one Aether connection's cached state from durable events,
// deltas and transcript pages (CONTRACTS.md §5.2).

import type { aether } from "@aether-ui/protocol";
import { invariant, object, omit, seq, string, type Json } from "./guards.ts";
import type { AetherConnState, StreamState, TranscriptState } from "./store.ts";

const RECENT_EVENTS = 500;
const MAX_STREAM_CHARS = 2_000_000;
/** Streams and accounting never change attention; anything else may (answers, archives, restarts). */
const NOT_ATTENTION = /^(agent\.message|agent\.usage_updated|invocation\..*|noesis\..*)$/;

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
    if (invocation && kind.role === "assistant") next = commitStream(next, agent, invocation);
  }
  if ((type === "invocation.completed" || type === "invocation.failed") && invocation) next = withStream(next, invocation, undefined);
  if (type === "noesis.settings_changed") next = { ...next, noesis: withNoesisSettings(next.noesis, kind) };
  invariant(next.lastSeq === at, "an applied event advances last_seq to its own seq");
  return next;
}

/** Lifecycle and usage events change the agent row; most others may change the attention snapshot. */
function markStale(state: AetherConnState, type: string, agent: string | undefined): AetherConnState {
  const agentStale = agent !== undefined && type.startsWith("agent.") && type !== "agent.message" && !state.staleAgents.includes(agent);
  const attentionStale = !NOT_ATTENTION.test(type) && !state.staleAttention;
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

/**
 * The committed assistant message replaces its invocation's tail: the stream stays as an
 * empty tombstone until the invocation ends, so a late delta cannot append the text again.
 */
function commitStream(state: AetherConnState, agent: string, invocation: string): AetherConnState {
  const stream = state.streams[invocation];
  invariant(!stream || stream.agent === agent, "an invocation belongs to one agent");
  const base = stream ?? emptyStream(agent, invocation, null);
  return withStream(state, invocation, { ...base, text: "", thinking: "", tools: [], recovering: false, partial: false, committed: true });
}

function emptyStream(agent: string, invocation: string, order: number | null): StreamState {
  return { agent, invocation, text: "", thinking: "", tools: [], lastInvocationSeq: order, recovering: false, partial: false, committed: false };
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
  if (previous?.committed) return { state, outcome: "ignored" };
  const lost: StreamState = { ...emptyStream(agent, invocation, order), recovering: true, partial: true };
  if (order !== null && previous?.lastInvocationSeq != null && order <= previous.lastInvocationSeq) return { state, outcome: "ignored" };
  if (previous?.recovering) return { state: withStream(state, invocation, { ...previous, lastInvocationSeq: order }), outcome: "ignored" };
  const expected = previous?.lastInvocationSeq == null ? 1 : previous.lastInvocationSeq + 1;
  if (order !== null && order !== expected) return { state: withStream(state, invocation, lost), outcome: "reread" };
  const stream: StreamState = { ...(previous ?? emptyStream(agent, invocation, order)), lastInvocationSeq: order };
  const grown = grow(stream, delta);
  if (grown === null) return { state: withStream(state, invocation, lost), outcome: "reread" };
  return { state: withStream(state, invocation, grown), outcome: "applied" };
}

/** Adds one delta's text, reasoning or tool input to the tail; null when it outgrows the cap. */
function grow(stream: StreamState, delta: Json): StreamState | null {
  if (delta.delta === "text" || delta.delta === "thinking") {
    const field = delta.delta;
    const text = stream[field] + String(delta.text ?? "");
    return text.length > MAX_STREAM_CHARS ? null : { ...stream, [field]: text };
  }
  if (delta.delta !== "tool_input") return stream;
  const id = typeof delta.tool_use_id === "string" ? delta.tool_use_id : null;
  const name = String(delta.name ?? "");
  const chunk = String(delta.partial_json ?? "");
  // A daemon without tool_use_id streams one call at a time, so its chunks join the last call.
  const at = id === null ? stream.tools.length - 1 : stream.tools.findIndex((tool) => tool.id === id);
  const target = at >= 0 && (id !== null || stream.tools[at]?.id === null) ? stream.tools[at] : undefined;
  const tools = [...stream.tools];
  if (target) tools[at] = { ...target, name: target.name || name, input: target.input + chunk };
  else tools.push({ id, name, input: chunk });
  const total = tools.reduce((sum, tool) => sum + tool.input.length, stream.text.length + stream.thinking.length);
  invariant(id === null || tools.filter((tool) => tool.id === id).length === 1, "one tail entry per tool_use_id");
  return total > MAX_STREAM_CHARS ? null : { ...stream, tools };
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
