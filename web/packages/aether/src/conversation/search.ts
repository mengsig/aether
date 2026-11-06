// Finding text in a transcript of any length: each row's searchable text is built once per row
// object, matches are counted per row, and the n-th match is found by binary search over the counts.

import { invariant } from "../fleet/invariant.ts";
import type { ConversationRow, Part } from "./rows.ts";
import { foldKey, orphanKey, toolKey } from "./toggles.ts";

const MIN_QUERY = 1;
/** Matches past this many per row are not counted: one huge tool output must not stall typing. */
const MAX_PER_ROW = 1_000;

/** `text` lower-cased one character at a time, with the offset each folded code unit came from, so a match maps back. */
export function foldCase(text: string): { readonly folded: string; readonly origin: readonly number[] } {
  let folded = "";
  const origin: number[] = [];
  let at = 0;
  for (const ch of text) {
    const low = ch.toLowerCase();
    for (let unit = 0; unit < low.length; unit += 1) origin.push(at);
    folded += low;
    at += ch.length;
  }
  origin.push(text.length);
  invariant(origin.length === folded.length + 1, "one origin per folded code unit, and the end");
  return { folded, origin };
}

/** The text a person could see in one part once it is open, lower-cased for matching. */
function partText(part: Part): string {
  switch (part.kind) {
    case "text": case "reasoning": case "compaction": return part.text;
    case "tool": return `${part.name}\n${part.input}\n${part.result?.text ?? ""}`;
    case "orphan-result": return part.result.text;
    case "attachment": return part.name;
  }
}

const texts = new WeakMap<ConversationRow, readonly string[]>();

function rowTexts(row: ConversationRow): readonly string[] {
  let cached = texts.get(row);
  if (cached === undefined) {
    cached = row.parts.map((part) => foldCase(partText(part)).folded);
    texts.set(row, cached);
  }
  return cached;
}

function countIn(text: string, needle: string, limit: number): number {
  let count = 0;
  for (let at = text.indexOf(needle); at >= 0 && count < limit; at = text.indexOf(needle, at + needle.length)) count += 1;
  return count;
}

export interface SearchResult {
  readonly query: string;
  /** Rows with at least one match, ascending. */
  readonly rows: readonly number[];
  /** `ends[i]` is the number of matches in `rows[0..i]`; its last value is the total. */
  readonly ends: readonly number[];
  readonly total: number;
}

export const NO_RESULT: SearchResult = { query: "", rows: [], ends: [], total: 0 };

export function searchRows(rows: readonly ConversationRow[], query: string): SearchResult {
  const needle = foldCase(query.trim()).folded;
  if (needle.length < MIN_QUERY) return NO_RESULT;
  const hits: number[] = [];
  const ends: number[] = [];
  let total = 0;
  rows.forEach((row, index) => {
    let count = 0;
    for (const text of rowTexts(row)) count += countIn(text, needle, MAX_PER_ROW - count);
    if (count === 0) return;
    total += count;
    hits.push(index);
    ends.push(total);
  });
  invariant(hits.length === ends.length && (ends.at(-1) ?? 0) === total, "every matching row has its running count");
  return { query: needle, rows: hits, ends, total };
}

/** Where match `n` (0-based) is: its row, and which match it is inside that row. */
export function locate(result: SearchResult, n: number): { row: number; nth: number } {
  invariant(Number.isInteger(n) && n >= 0 && n < result.total, `match ${n} of ${result.total} exists`);
  let low = 0;
  let high = result.ends.length - 1;
  while (low < high) {
    const mid = (low + high) >>> 1;
    if ((result.ends[mid] ?? 0) <= n) low = mid + 1;
    else high = mid;
  }
  const row = result.rows[low];
  invariant(row !== undefined, "a match lies in a matching row");
  return { row, nth: n - (low === 0 ? 0 : result.ends[low - 1] ?? 0) };
}

/** The folded parts of a row that hold the query: opening them shows the match. */
export function foldsToOpen(row: ConversationRow, query: string): string[] {
  const keys: string[] = [];
  const all = rowTexts(row);
  row.parts.forEach((part, index) => {
    if (!(all[index] ?? "").includes(query)) return;
    if (part.kind === "tool") keys.push(toolKey(row.key, index, part.id));
    else if (part.kind === "reasoning" || part.kind === "compaction") keys.push(foldKey(row.key, index));
    else if (part.kind === "orphan-result") keys.push(orphanKey(row.key, part.toolUseId));
  });
  return keys;
}

/** The next match after stepping by `step` from `current`, wrapping at both ends. */
export function stepMatch(current: number, step: 1 | -1, total: number): number {
  invariant(total > 0, "there is a match to step to");
  return (((current + step) % total) + total) % total;
}
