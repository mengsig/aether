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

export interface ConversationRow {
  /** `e:<seq>` for a committed entry, `s:<invocation>` for a live tail. */
  readonly key: string;
  readonly role: aether.MessageRole;
  readonly seq: number | null;
  readonly at: string | null;
  readonly invocation: string | null;
  readonly parts: readonly Part[];
  readonly live: LiveState | null;
  readonly meta: InvocationMeta | null;
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
    rows.push({
      key: `e:${entry.seq}`, role: entry.message.role, seq: entry.seq, at: entry.at, invocation, parts, live: null,
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
      key: `s:${stream.invocation}`, role: "assistant", seq: null, at: null, invocation: stream.invocation, parts,
      live: { partial: stream.partial, recovering: stream.recovering }, meta: null,
    });
  }
  rows.sort((a, b) => (a.key < b.key ? -1 : a.key > b.key ? 1 : 0));
  invariant(new Set(rows.map((row) => row.key)).size === rows.length, "one live tail per invocation");
  return rows;
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
