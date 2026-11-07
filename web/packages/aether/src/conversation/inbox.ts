// A durable user turn carries the inbox as the model read it (aether-context's `inbox_message`). The
// reader sees each delivery on its own: what a person typed, a message, a child's outcome or a note,
// or, when the boundaries between deliveries are in doubt, the whole turn verbatim.

import type { aether } from "@aether-ui/protocol";
import { invariant } from "../fleet/invariant.ts";

export type InboxSegment =
  | { readonly kind: "operator"; readonly text: string }
  | { readonly kind: "message"; readonly from: string; readonly text: string }
  | { readonly kind: "done"; readonly subject: string; readonly text: string }
  | { readonly kind: "failed"; readonly subject: string; readonly text: string }
  | { readonly kind: "approval"; readonly text: string }
  | { readonly kind: "notice"; readonly text: string }
  | { readonly kind: "verbatim"; readonly text: string };

/** The agent's inbox records (`get_inbox`) by seq: what each `[inbox n]` header really delivered. */
export interface Deliveries {
  readonly bySeq: ReadonlyMap<number, aether.InboxKind>;
  /** The highest inbox seq the reads asked past or returned; the next read asks only after it. */
  readonly cursor: number;
  /**
   * The transcript seq of the last turn written before a read: a delivery is written to the turn after
   * it arrives, so every delivery such a turn holds is in `bySeq`. A later turn cannot be checked yet.
   */
  readonly covers: number;
}

const HEADER = /^\[inbox (\d+)\] (.*)$/;
const OPERATOR_MARKER = " [authenticated operator instruction]";
/** The line `inbox_message` writes for each image a message carries, after its text. */
const IMAGE_LINE = /^\[image: /;

export const isInboxText = (text: string): boolean => HEADER.test(text.trimStart().split("\n", 1)[0] ?? "");

/** What one `[inbox n] …` header line opens, with the text it carries on that line. */
function opened(header: string): InboxSegment {
  const line = header.replace(OPERATOR_MARKER, "");
  let match = /^Message from (.+?):$/.exec(line.trimEnd());
  if (match?.[1] !== undefined) {
    const from = match[1];
    return from === "user" || from.startsWith("user:") ? { kind: "operator", text: "" } : { kind: "message", from, text: "" };
  }
  match = /^Task (\S+) completed: ([\s\S]*)$/.exec(line);
  if (match?.[1] !== undefined) return { kind: "done", subject: match[1], text: match[2] ?? "" };
  match = /^(?:Task|Child agent) (\S+) (?:failed: |was cancelled:? ?)([\s\S]*)$/.exec(line);
  if (match?.[1] !== undefined) return { kind: "failed", subject: match[1], text: match[2] ?? "" };
  if (line.startsWith("Approval ")) return { kind: "approval", text: line };
  return { kind: "notice", text: line };
}

/** Whether `record` is what a header that opened `segment` says it is; `line` is the header after `[inbox n] `. */
function matches(segment: InboxSegment, line: string, record: aether.InboxKind): boolean {
  invariant(!HEADER.test(line), "a header is matched by its text after the seq");
  switch (segment.kind) {
    case "operator": return record.kind === "message" && record.from.actor === "user" && line.endsWith(OPERATOR_MARKER);
    case "message": return record.kind === "message" && record.from.actor !== "user" && !line.endsWith(OPERATOR_MARKER);
    case "done": return record.kind === "task_completed" && record.task === segment.subject;
    case "failed": return (record.kind === "task_failed" || record.kind === "task_cancelled") ? record.task === segment.subject
      : record.kind === "child_failed" && record.agent === segment.subject;
    case "approval": return record.kind === "approval_resolved" && line.startsWith(`Approval ${record.approval} `);
    case "notice": return !["message", "task_completed", "task_failed", "task_cancelled", "approval_resolved", "child_failed"].includes(record.kind);
    case "verbatim": invariant(false, "a header never opens a verbatim segment");
  }
}

/** Whether an operator segment's text is exactly the message its record holds (images named after). */
function operatorIntact(segment: InboxSegment, record: aether.InboxKind | undefined): boolean {
  if (segment.kind !== "operator") return true;
  invariant(record?.kind === "message", "an operator delivery is confirmed by its message record");
  const body = record.content.map((block) => (block.type === "text" ? block.text : "")).join("").trim();
  const lines = segment.text.trim().split("\n");
  while (lines.length > 0 && IMAGE_LINE.test(lines.at(-1) ?? "")) lines.pop();
  return lines.join("\n").trim() === body;
}

interface Opened {
  readonly seq: number;
  readonly segment: InboxSegment;
}

/**
 * The deliveries in an inbox turn. With the agent's records, a header line opens a delivery only when
 * a record of that seq and kind exists and seqs ascend; any other header line is quoted text of the
 * delivery it sits in, and an operator's text must equal its record. `records` must cover the turn
 * (`Deliveries.covers`). Without them, or when any boundary is in doubt, it stays one verbatim segment.
 */
export function splitInbox(text: string, records: Deliveries | null = null): readonly InboxSegment[] {
  const trimmed = text.trim();
  if (!isInboxText(text)) return trimmed === "" ? [] : [{ kind: "operator", text: trimmed }];
  const verbatim: readonly InboxSegment[] = [{ kind: "verbatim", text: trimmed }];
  if (records === null) return verbatim;
  const parts = splitByRecords(trimmed, records.bySeq);
  if (parts === null || !parts.every((part) => operatorIntact(part.segment, records.bySeq.get(part.seq)))) return verbatim;
  const kept = parts.map((part) => ({ ...part.segment, text: part.segment.text.trim() })).filter((s) => s.kind !== "operator" || s.text !== "");
  invariant(kept.length <= parts.length, "splitting only drops empty operator turns");
  return kept;
}

const appended = (segment: InboxSegment, line: string): InboxSegment => ({ ...segment, text: segment.text === "" ? line : `${segment.text}\n${line}` });

/** With records: only a header its record confirms opens a delivery; the first must be confirmed. */
function splitByRecords(text: string, records: Deliveries["bySeq"]): Opened[] | null {
  const segments: Opened[] = [];
  for (const line of text.split("\n")) {
    const header = HEADER.exec(line);
    const last = segments.at(-1);
    const seq = Number(header?.[1]);
    const record = records.get(seq);
    const said = header?.[2];
    const segment = said === undefined ? null : opened(said);
    if (segment !== null && said !== undefined && record !== undefined && (last === undefined || seq > last.seq) && matches(segment, said, record)) {
      segments.push({ seq, segment });
      continue;
    }
    if (last === undefined) return null;
    segments[segments.length - 1] = { seq: last.seq, segment: appended(last.segment, line) };
  }
  invariant(segments.every((s, i) => i === 0 || s.seq > (segments[i - 1]?.seq ?? -1)), "confirmed deliveries ascend by seq");
  return segments;
}

/**
 * `base` with a later `get_inbox` read added: the items after inbox seq `after`, read once every
 * transcript turn up to seq `covers` was written. Only seqs the daemon returned move the cursor.
 */
export function withDeliveries(base: Deliveries | null, read: { readonly items: readonly aether.InboxItem[]; readonly after: number; readonly covers: number }, agent: string): Deliveries {
  invariant(Number.isSafeInteger(read.after) && read.after >= 0 && (base === null || read.after === base.cursor), "a read asks after a whole seq, the cursor when it adds to one");
  invariant(Number.isSafeInteger(read.covers) && read.covers >= 0, "a read covers turns up to a whole transcript seq");
  const bySeq = new Map(base?.bySeq);
  let cursor = Math.max(base?.cursor ?? 0, read.after);
  for (const item of read.items) {
    invariant(item.agent_id === agent, "get_inbox answers with the agent's own items");
    invariant(item.seq > read.after && !bySeq.has(item.seq), "an inbox seq after the cursor names one item");
    bySeq.set(item.seq, item.kind);
    cursor = Math.max(cursor, item.seq);
  }
  return { bySeq, cursor, covers: Math.max(base?.covers ?? 0, read.covers) };
}

/** The lowest and highest `[inbox n]` seq in a turn's text, or null for none. */
export function seqRange(text: string): readonly [low: number, high: number] | null {
  let range: [number, number] | null = null;
  for (const line of text.split("\n")) {
    const seq = HEADER.exec(line)?.[1];
    if (seq === undefined) continue;
    const n = Number(seq);
    range = range === null ? [n, n] : [Math.min(range[0], n), Math.max(range[1], n)];
  }
  invariant(range === null || range[0] <= range[1], "a seq range is ordered");
  return range;
}
