// The transcript as rows: committed entries from core's store with tool calls paired to their
// results by tool_use_id, then each invocation's live tail until its message commits.

import type { StreamState } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { invariant } from "../fleet/invariant.ts";

export interface ToolResultView {
  readonly text: string;
  readonly isError: boolean;
  readonly images: number;
}

export type Part =
  | { readonly kind: "text"; readonly text: string }
  | { readonly kind: "reasoning"; readonly text: string; readonly redacted: boolean }
  | { readonly kind: "tool"; readonly id: string | null; readonly name: string; readonly input: string; readonly result: ToolResultView | null }
  | { readonly kind: "orphan-result"; readonly toolUseId: string; readonly result: ToolResultView }
  | { readonly kind: "attachment"; readonly name: string; readonly mediaType: string; readonly sizeBytes: number | null }
  | { readonly kind: "compaction"; readonly text: string };

/** What `invocation.completed` or `invocation.failed` said about the invocation behind a message. */
export interface InvocationMeta {
  readonly model: string | null;
  readonly usage: aether.Usage | null;
  readonly durationMs: number | null;
  readonly error: string | null;
}

export interface LiveState {
  readonly partial: boolean;
  readonly recovering: boolean;
}

/** Who a row speaks for: the operator, the agent, another agent writing to it, or a system notice. */
export type SpeakerKind = "user" | "assistant" | "agent" | "notice";

export interface Speaker {
  readonly kind: SpeakerKind;
  /** The sender the inbox named (`user:…`, an agent id), or null when the message names none. */
  readonly from: string | null;
}

export interface ConversationRow {
  /** `e:<seq>` for a committed entry, `s:<invocation>` for a live tail. */
  readonly key: string;
  readonly role: aether.MessageRole;
  readonly speaker: Speaker;
  readonly seq: number | null;
  readonly at: string | null;
  readonly invocation: string | null;
  readonly parts: readonly Part[];
  readonly live: LiveState | null;
  readonly meta: InvocationMeta | null;
  /** Sent from the composer and not yet delivered by the daemon; drawn after every other row. */
  readonly queued?: true;
}

/** A message the composer sent that the daemon has not yet delivered to the agent. */
export interface QueuedMessage {
  readonly text: string;
  readonly at: string;
  /** The newest committed seq when it was sent; its delivery commits after this. */
  readonly afterSeq: number;
}

export interface Pairing {
  readonly results: ReadonlyMap<string, ToolResultView>;
  readonly uses: ReadonlySet<string>;
  /** Invocations whose assistant message is committed: their live tails are replaced. */
  readonly committed: ReadonlySet<string>;
}

export function resultView(block: aether.ContentBlockToolResult): ToolResultView {
  const texts: string[] = [];
  let images = 0;
  for (const part of block.content) {
    if (part.type === "text") texts.push(part.text);
    else images += 1;
  }
  return { text: texts.join("\n"), isError: block.is_error === true, images };
}

/** One pass over the transcript: every tool result by its tool_use_id, every call id, every committed invocation. */
export function pairTools(entries: readonly aether.TranscriptEntry[]): Pairing {
  const results = new Map<string, ToolResultView>();
  const uses = new Set<string>();
  const committed = new Set<string>();
  for (const entry of entries) {
    if (entry.message.role === "assistant" && entry.invocation) committed.add(entry.invocation);
    for (const block of entry.message.content) {
      if (block.type === "tool_use") uses.add(block.id);
      else if (block.type === "tool_result") results.set(block.tool_use_id, resultView(block));
    }
  }
  invariant(committed.size <= entries.length, "each committed invocation has its own entry");
  return { results, uses, committed };
}

export function prettyInput(input: unknown): string {
  if (typeof input === "string") return input;
  return JSON.stringify(input, null, 2) ?? "";
}

function blockPart(block: aether.ContentBlock, pairing: Pairing): Part | null {
  switch (block.type) {
    case "text": return block.text === "" ? null : { kind: "text", text: block.text };
    case "thinking": return { kind: "reasoning", text: block.text, redacted: block.redacted === true };
    case "tool_use": return { kind: "tool", id: block.id, name: block.name, input: prettyInput(block.input), result: pairing.results.get(block.id) ?? null };
    // A result shows inside its call; only a result whose call is not in the transcript stands alone.
    case "tool_result": return pairing.uses.has(block.tool_use_id) ? null : { kind: "orphan-result", toolUseId: block.tool_use_id, result: resultView(block) };
    case "image": return { kind: "attachment", name: "Inline image", mediaType: block.media_type, sizeBytes: null };
    case "image_ref": return { kind: "attachment", name: block.name ?? block.id, mediaType: block.media_type, sizeBytes: block.size_bytes };
    case "compaction": return { kind: "compaction", text: block.content };
    default: return null;
  }
}

const INBOX_LINE = /^\[inbox \d+\] /;
const MESSAGE_FROM = /^\[inbox \d+\] Message from (.+):(?: \[authenticated operator instruction\])?$/;
const ASSISTANT: Speaker = { kind: "assistant", from: null };
const OPERATOR: Speaker = { kind: "user", from: null };
const NOTICE: Speaker = { kind: "notice", from: null };

/** One inbox item as the daemon wrote it: who sent it (null for a notice) and the text under its header. */
interface InboxItem { readonly from: string | null; readonly body: string }

const isOperator = (from: string): boolean => from === "user" || from.startsWith("user:");

/** Splits an inbox item at its header line; the text must start with one. */
function inboxItem(text: string): InboxItem {
  invariant(INBOX_LINE.test(text), "an inbox item starts with its header");
  const newline = text.indexOf("\n");
  const header = newline < 0 ? text : text.slice(0, newline);
  const from = MESSAGE_FROM.exec(header)?.[1] ?? null;
  invariant(from !== "", "an inbox header names its sender");
  return { from, body: newline < 0 ? "" : text.slice(newline + 1) };
}

/**
 * The speaker of a user turn, read from the inbox header the daemon writes before a delivered message
 * ("[inbox 3] Message from user:…:"); a single message's header moves into the row's head.
 */
export function speakerOf(role: aether.MessageRole, parts: readonly Part[]): { speaker: Speaker; parts: readonly Part[] } {
  if (role === "assistant") return { speaker: ASSISTANT, parts };
  const first = parts[0];
  if (first === undefined || parts.every((part) => part.kind === "orphan-result")) return { speaker: NOTICE, parts };
  if (first.kind !== "text" || !INBOX_LINE.test(first.text)) return { speaker: OPERATOR, parts };
  const { from, body: rest } = inboxItem(first.text);
  if (from === null) return { speaker: NOTICE, parts };
  const speaker: Speaker = { kind: isOperator(from) ? "user" : "agent", from };
  // Several inbox items in one turn keep their headers: each says what it is.
  if (/^\[inbox \d+\] /m.test(rest)) return { speaker, parts };
  const body = rest.trim();
  const remaining = body === "" ? parts.slice(1) : [{ kind: "text", text: body } as const, ...parts.slice(1)];
  return { speaker, parts: remaining.length === 0 ? parts : remaining };
}

/** Committed rows in seq order; a message left with nothing of its own to show has no row. */
export function entryRows(entries: readonly aether.TranscriptEntry[], pairing: Pairing, meta: ReadonlyMap<string, InvocationMeta>): ConversationRow[] {
  const rows: ConversationRow[] = [];
  let last = -Infinity;
  for (const entry of entries) {
    invariant(entry.seq > last, `transcript entries ascend by seq (${entry.seq} after ${last})`);
    last = entry.seq;
    const parts: Part[] = [];
    for (const block of entry.message.content) {
      const part = blockPart(block, pairing);
      if (part) parts.push(part);
    }
    if (parts.length === 0) continue;
    const invocation = entry.invocation ?? null;
    const spoken = speakerOf(entry.message.role, parts);
    rows.push({
      key: `e:${entry.seq}`, role: entry.message.role, speaker: spoken.speaker, seq: entry.seq, at: entry.at, invocation, parts: spoken.parts, live: null,
      meta: invocation === null ? null : meta.get(invocation) ?? null,
    });
  }
  return rows;
}

/** The live tails of one agent whose message has not committed, oldest invocation first. */
export function liveRows(streams: Readonly<Record<string, StreamState>>, agent: string, committed: ReadonlySet<string>): ConversationRow[] {
  const rows: ConversationRow[] = [];
  for (const stream of Object.values(streams)) {
    if (stream.agent !== agent || stream.committed || committed.has(stream.invocation)) continue;
    const parts: Part[] = [];
    if (stream.thinking !== "") parts.push({ kind: "reasoning", text: stream.thinking, redacted: false });
    if (stream.text !== "") parts.push({ kind: "text", text: stream.text });
    for (const tool of stream.tools) parts.push({ kind: "tool", id: tool.id, name: tool.name, input: tool.input, result: null });
    if (parts.length === 0 && !stream.recovering) continue;
    rows.push({
      key: `s:${stream.invocation}`, role: "assistant", speaker: ASSISTANT, seq: null, at: null, invocation: stream.invocation, parts,
      live: { partial: stream.partial, recovering: stream.recovering }, meta: null,
    });
  }
  rows.sort((a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : 0));
  invariant(new Set(rows.map((row) => row.key)).size === rows.length, "one live tail per invocation");
  return rows;
}

/** Rows for the messages still queued, in the order they were sent. */
export function queuedRows(queued: readonly QueuedMessage[]): ConversationRow[] {
  return queued.map((message, index) => {
    invariant(message.text.trim() !== "", "a queued message has text");
    invariant(message.afterSeq >= 0, "a queued message follows a committed seq");
    return {
      key: `q:${message.afterSeq}:${index}`, role: "user", speaker: OPERATOR, seq: null, at: message.at, invocation: null,
      parts: [{ kind: "text", text: message.text }], live: null, meta: null, queued: true,
    };
  });
}

const textOf = (content: readonly aether.ContentBlock[]): string =>
  content.filter((block): block is aether.ContentBlockText => block.type === "text").map((block) => block.text).join("\n").trim();

/** The inbox seq the daemon gave the operator's message after the send, or null without that event. */
function inboxSeqOf(events: readonly aether.Event[], agent: string, message: QueuedMessage): number | null {
  for (const event of events) {
    if (event.agent_id !== agent || event.seq <= message.afterSeq || event.kind.type !== "inbox.delivered") continue;
    const item = event.kind.item;
    if (item.kind.kind === "message" && item.kind.from.actor === "user" && textOf(item.kind.content) === message.text) return item.seq;
  }
  return null;
}

/** Whether the agent's turn read its inbox up to `seq`: the daemon commits that with the user turn. */
function isConsumed(events: readonly aether.Event[], agent: string, seq: number): boolean {
  invariant(seq > 0, "inbox seqs start at one");
  return events.some((event) => event.agent_id === agent && event.kind.type === "inbox.consumed" && event.kind.up_to >= seq);
}

/**
 * Whether the daemon delivered the queued message: its inbox item was consumed, by the events. Without
 * the delivery event (an older daemon, or a long turn pushed it out of the recent events), the
 * operator's own user row committed after the send carries the text.
 */
export function isDelivered(rows: readonly ConversationRow[], events: readonly aether.Event[], agent: string, message: QueuedMessage): boolean {
  invariant(message.text !== "", "a queued message has text");
  invariant(agent !== "", "a queued message names its agent");
  const seq = inboxSeqOf(events, agent, message);
  if (seq !== null) return isConsumed(events, agent, seq);
  return rows.some((row) => row.seq !== null && row.seq > message.afterSeq && row.speaker.kind === "user"
    && row.parts.some((part) => part.kind === "text" && part.text.includes(message.text)));
}

const modelOf = (ref: aether.ModelRef): string => `${ref.provider}/${ref.model}`;

/** Model, cost and timing per invocation of this agent, from the durable events core keeps. */
export function invocationMeta(events: readonly aether.Event[], agent: string): ReadonlyMap<string, InvocationMeta> {
  const meta = new Map<string, InvocationMeta>();
  for (const event of events) {
    if (event.agent_id !== agent) continue;
    const kind = event.kind;
    if (kind.type === "invocation.completed") {
      meta.set(kind.invocation, { model: modelOf(kind.model), usage: kind.usage, durationMs: kind.duration_ms, error: null });
    } else if (kind.type === "invocation.failed") {
      meta.set(kind.invocation, { model: null, usage: null, durationMs: null, error: kind.error });
    }
  }
  return meta;
}

/** How one tool call went, from its `tool.*` events: keyed by the call's tool_use_id. */
export interface ToolActivity {
  readonly state: "running" | "ok" | "error";
  readonly durationMs: number | null;
}

/** Each of this agent's tool calls by its provider (tool_use) id, as its events last left it. */
export function toolActivity(events: readonly aether.Event[], agent: string): ReadonlyMap<string, ToolActivity> {
  const providerIds = new Map<string, string>();
  const activity = new Map<string, ToolActivity>();
  const set = (call: string, value: ToolActivity) => {
    const id = providerIds.get(call);
    if (id !== undefined) activity.set(id, value);
  };
  for (const event of events) {
    if (event.agent_id !== agent) continue;
    const kind = event.kind;
    if (kind.type === "tool.requested") providerIds.set(kind.call.id, kind.call.provider_call_id);
    else if (kind.type === "tool.started") set(kind.call, { state: "running", durationMs: null });
    else if (kind.type === "tool.succeeded") set(kind.call, { state: kind.outcome.is_error === true ? "error" : "ok", durationMs: kind.outcome.duration_ms ?? null });
    else if (kind.type === "tool.failed") set(kind.call, { state: "error", durationMs: kind.duration_ms ?? null });
  }
  invariant(activity.size <= providerIds.size, "activity is only kept for requested calls");
  return activity;
}

/** Committed messages and tool calls, for the transcript's header. */
export function rowCounts(rows: readonly ConversationRow[]): { messages: number; tools: number } {
  let messages = 0;
  let tools = 0;
  for (const row of rows) {
    if (row.live === null) messages += 1;
    for (const part of row.parts) if (part.kind === "tool") tools += 1;
  }
  invariant(messages <= rows.length, "a row is at most one message");
  return { messages, tools };
}
