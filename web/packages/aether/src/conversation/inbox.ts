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
  /** The highest seq read; a turn naming a higher one arrived after the read, so it cannot be checked. */
  readonly last: number;
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

/** Whether `record` is what a header that opened `segment` (its whole line `line`) says it is. */
function matches(segment: InboxSegment, line: string, record: aether.InboxKind): boolean {
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
 * delivery it sits in, and an operator's text must equal its record. Without records, or when any
 * boundary is in doubt, the turn stays one verbatim segment: no line of anyone's text is lost.
 */
export function splitInbox(text: string, records: Deliveries | null = null): readonly InboxSegment[] {
  const trimmed = text.trim();
  if (!isInboxText(text)) return trimmed === "" ? [] : [{ kind: "operator", text: trimmed }];
  const verbatim: readonly InboxSegment[] = [{ kind: "verbatim", text: trimmed }];
  const known = records !== null && highestSeq(trimmed) <= records.last ? records.bySeq : null;
  const parts = known === null ? splitByText(trimmed) : splitByRecords(trimmed, known);
  if (parts === null) return verbatim;
  if (known !== null && !parts.every((part) => operatorIntact(part.segment, known.get(part.seq)))) return verbatim;
  const kept = parts.map((part) => ({ ...part.segment, text: part.segment.text.trim() })).filter((s) => s.kind !== "operator" || s.text !== "");
  invariant(kept.length <= parts.length, "splitting only drops empty operator turns");
  return kept;
}

const appended = (segment: InboxSegment, line: string): InboxSegment => ({ ...segment, text: segment.text === "" ? line : `${segment.text}\n${line}` });

/** Without records: seqs must ascend and nothing may follow the operator's text, or it is in doubt. */
function splitByText(text: string): Opened[] | null {
  const segments: Opened[] = [];
  for (const line of text.split("\n")) {
    const header = HEADER.exec(line);
    const last = segments.at(-1);
    if (header?.[2] === undefined) {
      invariant(last !== undefined, "inbox text starts with a header");
      segments[segments.length - 1] = { seq: last.seq, segment: appended(last.segment, line) };
      continue;
    }
    const seq = Number(header[1]);
    if (last !== undefined && (seq <= last.seq || last.segment.kind === "operator")) return null;
    segments.push({ seq, segment: opened(header[2]) });
  }
  return segments;
}

/** With records: only a header its record confirms opens a delivery; the first must be confirmed. */
function splitByRecords(text: string, records: Deliveries["bySeq"]): Opened[] | null {
  const segments: Opened[] = [];
  for (const line of text.split("\n")) {
    const header = HEADER.exec(line);
    const last = segments.at(-1);
    const seq = Number(header?.[1]);
    const record = records.get(seq);
    const segment = header?.[2] === undefined ? null : opened(header[2]);
    if (segment !== null && record !== undefined && (last === undefined || seq > last.seq) && matches(segment, line, record)) {
      segments.push({ seq, segment });
      continue;
    }
    if (last === undefined) return null;
    segments[segments.length - 1] = { seq: last.seq, segment: appended(last.segment, line) };
  }
  invariant(segments.every((s, i) => i === 0 || s.seq > (segments[i - 1]?.seq ?? -1)), "confirmed deliveries ascend by seq");
  return segments;
}

/** The records by seq, from a `get_inbox` reply; null when there is none to trust. */
export function deliveries(items: readonly aether.InboxItem[] | null, agent: string): Deliveries | null {
  if (items === null) return null;
  const bySeq = new Map<number, aether.InboxKind>();
  let last = 0;
  for (const item of items) {
    invariant(item.agent_id === agent, "get_inbox answers with the agent's own items");
    invariant(!bySeq.has(item.seq), "an inbox seq names one item");
    bySeq.set(item.seq, item.kind);
    last = Math.max(last, item.seq);
  }
  return { bySeq, last };
}

/** The highest `[inbox n]` seq in a turn's text, or 0; a seq above every record means they are stale. */
export function highestSeq(text: string): number {
  let highest = 0;
  for (const line of text.split("\n")) {
    const seq = Number(HEADER.exec(line)?.[1] ?? 0);
    if (seq > highest) highest = seq;
  }
  return highest;
}
