// An evidence bundle: what the frontend gathered from reads the user can already make, measured and
// trimmed here so a refused bundle is never sent. The daemon checks the same 32 KiB limit again.

import type { aether, EntityKind } from "@aether-ui/protocol";
import { invariant } from "../learn/invariant.ts";

/** The most excerpt text one bundle may carry, in UTF-8 bytes, as the daemon counts it. */
export const BUNDLE_LIMIT_BYTES = 32_768;
/** One excerpt's share, so a normal bundle of a dozen facts fits with room to spare. */
export const EXCERPT_MAX_BYTES = 2_048;
/** A brief or instructions carry the most context, so they may run longer. */
export const BRIEF_MAX_BYTES = 6_144;
/** The daemon's other bounds: items per bundle and title length in characters. */
export const BUNDLE_MAX_ITEMS = 50;
export const TITLE_MAX_CHARS = 300;

/** The entities a bundle is built from; each has a source in sources.ts. */
export const UNDERSTAND_KINDS = ["item", "run", "agent", "pull_request"] as const satisfies readonly EntityKind[];
export type UnderstandKind = (typeof UNDERSTAND_KINDS)[number];

export interface UnderstandRequest {
  readonly kind: UnderstandKind;
  /** The entity's own connection: an Archon one for items, runs and PRs, an Aether one for agents. */
  readonly conn: string;
  readonly id: string;
}

export const EVIDENCE_STATUSES: readonly aether.EvidenceStatus[] = ["verified", "reviewed", "preliminary"];

const ENCODER = new TextEncoder();

export function utf8Bytes(text: string): number {
  return ENCODER.encode(text).length;
}

function codePointBytes(point: number): number {
  if (point < 0x80) return 1;
  if (point < 0x800) return 2;
  return point < 0x10000 ? 3 : 4;
}

/** Cuts `text` to at most `max` UTF-8 bytes on a character boundary, ending a cut with "…". */
export function trimToBytes(text: string, max: number): string {
  invariant(Number.isInteger(max) && max >= 3, "an excerpt keeps room for the ellipsis");
  if (utf8Bytes(text) <= max) return text;
  const budget = max - utf8Bytes("…");
  let used = 0;
  let kept = "";
  for (const char of text) {
    const size = codePointBytes(char.codePointAt(0) ?? 0);
    if (used + size > budget) break;
    used += size;
    kept += char;
  }
  const trimmed = `${kept.trimEnd()}…`;
  invariant(utf8Bytes(trimmed) <= max, "a trimmed excerpt fits its share");
  return trimmed;
}

/** Plain text: one newline style, no control characters, at most one blank line in a row. */
export function plainText(text: string): string {
  return text
    .replace(/\r\n?/g, "\n")
    // eslint-disable-next-line no-control-regex -- removing control characters is the point
    .replace(/[\u0000-\u0008\u000b-\u001f\u007f]/g, "")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

/** One evidence item: its excerpt as trimmed plain text, its status always set. */
export function evidence(ref: aether.EvidenceRef, status: aether.EvidenceStatus, text: string, max = EXCERPT_MAX_BYTES): aether.EvidenceItem {
  invariant(ref.id.length > 0, "evidence names what it is about");
  invariant(EVIDENCE_STATUSES.includes(status), `evidence status ${String(status)} is known`);
  const excerpt = trimToBytes(plainText(text), max);
  return { ref: ref.url ? ref : { kind: ref.kind, id: ref.id }, excerpt, status };
}

/** A bundle titled with at most the characters the daemon keeps, cut with "…". */
export function bundleOf(title: string, items: aether.EvidenceItem[]): aether.EvidenceBundle {
  const chars = [...plainText(title).replace(/\s+/g, " ")];
  const clamped = chars.length <= TITLE_MAX_CHARS ? chars.join("") : `${chars.slice(0, TITLE_MAX_CHARS - 1).join("").trimEnd()}…`;
  invariant(clamped.length > 0, "a bundle has a title");
  invariant([...clamped].length <= TITLE_MAX_CHARS, "a bundle title fits the daemon's bound");
  return { title: clamped, items };
}

export type BundleCheck =
  | { readonly ok: true; readonly bytes: number }
  | { readonly ok: false; readonly bytes: number; readonly reason: string };

/** Measures every excerpt in UTF-8 bytes and refuses a bundle over 32 KiB, over 50 facts, or with none. */
export function checkBundle(bundle: aether.EvidenceBundle): BundleCheck {
  invariant(bundle.title.trim().length > 0, "a bundle has a title");
  invariant(bundle.items.every((item) => EVIDENCE_STATUSES.includes(item.status)), "every evidence item keeps its status");
  const bytes = bundle.items.reduce((total, item) => total + utf8Bytes(item.excerpt), 0);
  if (bundle.items.length === 0) return { ok: false, bytes, reason: "There is nothing here to learn from yet." };
  if (bundle.items.length > BUNDLE_MAX_ITEMS) {
    return { ok: false, bytes, reason: `The evidence has ${bundle.items.length} facts, more than the ${BUNDLE_MAX_ITEMS} a session may start from.` };
  }
  if (bytes > BUNDLE_LIMIT_BYTES) {
    return { ok: false, bytes, reason: `The evidence is ${formatBytes(bytes)}, over the ${formatBytes(BUNDLE_LIMIT_BYTES)} a session may start from.` };
  }
  return { ok: true, bytes };
}

export function formatBytes(bytes: number): string {
  invariant(Number.isFinite(bytes) && bytes >= 0, "a size is a nonnegative number");
  return bytes < 1024 ? `${bytes} B` : `${(bytes / 1024).toFixed(1)} KiB`;
}

const QUERY_KEY = "understand";

/** `#/learn?understand=<kind>&conn=<conn>&id=<id>`: routes are frozen, so the sheet rides on F5's `#/learn`. */
export function understandHref(request: UnderstandRequest): string {
  invariant(request.conn.length > 0 && request.id.length > 0, "an understand link names a connection and an entity");
  const query = new URLSearchParams({ [QUERY_KEY]: request.kind, conn: request.conn, id: request.id });
  return `#/learn?${query.toString()}`;
}

/** The request a hash carries, or null when it asks for none or names an unknown kind. */
export function parseUnderstand(hash: string): UnderstandRequest | null {
  const at = hash.indexOf("?");
  if (at < 0) return null;
  const query = new URLSearchParams(hash.slice(at + 1));
  const kind = query.get(QUERY_KEY);
  const conn = query.get("conn") ?? "";
  const id = query.get("id") ?? "";
  if (kind === null || !(UNDERSTAND_KINDS as readonly string[]).includes(kind)) return null;
  if (conn === "" || id === "") return null;
  return { kind: kind as UnderstandKind, conn, id };
}
