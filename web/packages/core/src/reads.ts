// Read models core does not sync by events (memories, devices, providers, rules, …): each reread's
// reply is stored whole under (connection, key), so screens derive from the store and keep no copy.

import type { ConnId } from "@aether-ui/protocol";
import { invariant, type Json } from "./guards.ts";
import type { State } from "./store.ts";

export interface ReadEntry {
  /** The reply's `data`, whole; null until the first reread succeeds. */
  readonly value: unknown;
  /** The daemon's own stamp (`as_of` or `last_seq` in the reply), when it gives one. */
  readonly asOf: number | null;
  /** The order the reread that produced `value` was issued in. */
  readonly issued: number;
  /** The newest reread issued for this key; above `issued` while one is in flight. */
  readonly requested: number;
  readonly error: string | null;
}

/** The stamp a reply carries about itself, if any. */
export function stampOf(data: unknown): number | null {
  if (data === null || typeof data !== "object" || Array.isArray(data)) return null;
  const record = data as Json;
  const stamp = typeof record.as_of === "number" ? record.as_of : record.last_seq;
  return typeof stamp === "number" && Number.isSafeInteger(stamp) ? stamp : null;
}

/** Whether a reply replaces the held one: never an older over a newer, by daemon stamp, else by issue order. */
export function readReplaces(incoming: { asOf: number | null; issued: number }, held: ReadEntry | undefined): boolean {
  if (held === undefined || held.issued === 0) return true;
  if (incoming.asOf !== null && held.asOf !== null && incoming.asOf !== held.asOf) return incoming.asOf > held.asOf;
  return incoming.issued >= held.issued;
}

const EMPTY_ENTRY: ReadEntry = Object.freeze({ value: null, asOf: null, issued: 0, requested: 0, error: null });

function put(state: State, conn: ConnId, key: string, recipe: (held: ReadEntry | undefined) => ReadEntry | undefined): State {
  const forConn = state.reads[conn] ?? {};
  const held = forConn[key];
  const next = recipe(held);
  if (next === held || next === undefined) return state;
  return { ...state, reads: { ...state.reads, [conn]: { ...forConn, [key]: next } } };
}

export function markRequested(state: State, conn: ConnId, key: string, issued: number): State {
  invariant(issued > 0, "a reread's issue number is positive");
  return put(state, conn, key, (held) => ({ ...(held ?? EMPTY_ENTRY), requested: Math.max(held?.requested ?? 0, issued) }));
}

export function applyRead(state: State, conn: ConnId, key: string, issued: number, value: unknown): State {
  const asOf = stampOf(value);
  return put(state, conn, key, (held) => {
    if (!readReplaces({ asOf, issued }, held)) return held;
    return { value, asOf, issued, requested: Math.max(held?.requested ?? 0, issued), error: null };
  });
}

/** A failed reread keeps the last good value and shows its error, unless a newer reread already answered. */
export function failRead(state: State, conn: ConnId, key: string, issued: number, error: string): State {
  invariant(error.length > 0, "a failed reread says why");
  return put(state, conn, key, (held) => (held !== undefined && held.issued > issued ? held : { ...(held ?? EMPTY_ENTRY), error }));
}

/** Removes one submission from the outbox and the review, once the operator settled what became of it. */
export function settleSubmission(state: State, conn: ConnId, key: string): State {
  invariant(key.length > 0, "a submission is settled by its key");
  const review = state.review.filter((s) => !(s.conn === conn && s.key === key));
  const aether = state.aether[conn];
  const archon = state.archon[conn];
  const drop = <T extends { outbox: readonly { key: string }[] }>(s: T): T => ({ ...s, outbox: s.outbox.filter((x) => x.key !== key) });
  return {
    ...state, review,
    aether: aether ? { ...state.aether, [conn]: drop(aether) } : state.aether,
    archon: archon ? { ...state.archon, [conn]: drop(archon) } : state.archon,
  };
}
