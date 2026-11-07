// A durable user turn carries the inbox as the model read it (aether-context's `inbox_message`). The
// reader sees each delivery on its own: what a person typed, a message, a child's outcome or a note,
// or, when a body may quote a header, the whole turn verbatim.

import { invariant } from "../fleet/invariant.ts";

export type InboxSegment =
  | { readonly kind: "operator"; readonly text: string }
  | { readonly kind: "message"; readonly from: string; readonly text: string }
  | { readonly kind: "done"; readonly subject: string; readonly text: string }
  | { readonly kind: "failed"; readonly subject: string; readonly text: string }
  | { readonly kind: "approval"; readonly text: string }
  | { readonly kind: "notice"; readonly text: string }
  | { readonly kind: "verbatim"; readonly text: string };

const HEADER = /^\[inbox (\d+)\] (.*)$/;
const OPERATOR_MARKER = " [authenticated operator instruction]";

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

function appended(segment: InboxSegment, line: string): InboxSegment {
  const text = segment.text === "" ? line : `${segment.text}\n${line}`;
  return { ...segment, text };
}

/**
 * The deliveries in an inbox turn, split on its headers. Bodies are not escaped, so where one may quote a
 * header (seqs out of order, or any header after the operator's message) the turn stays one verbatim
 * segment: no line of anyone's text is dropped, reattributed or folded into another delivery.
 */
export function splitInbox(text: string): readonly InboxSegment[] {
  if (!isInboxText(text)) return text.trim() === "" ? [] : [{ kind: "operator", text: text.trim() }];
  const segments: InboxSegment[] = [];
  let current: InboxSegment | null = null;
  let seq = -1;
  for (const line of text.trim().split("\n")) {
    const header = HEADER.exec(line);
    if (header?.[2] !== undefined) {
      if (Number(header[1]) <= seq || current?.kind === "operator") return [{ kind: "verbatim", text: text.trim() }];
      if (current !== null) segments.push(current);
      seq = Number(header[1]);
      current = opened(header[2]);
      continue;
    }
    invariant(current !== null, "inbox text starts with a header");
    current = appended(current, line);
  }
  if (current !== null) segments.push(current);
  const kept = segments.map((s) => ({ ...s, text: s.text.trim() })).filter((s) => s.kind !== "operator" || s.text !== "");
  invariant(kept.length <= segments.length, "splitting only drops empty operator turns");
  return kept;
}
