// Unsent text, kept per connection and agent (and per question) in sessionStorage, so leaving a
// screen does not lose it. A draft is cleared only once its delivery is confirmed.

import type { ConnId } from "@aether-ui/protocol";
import { useCallback, useRef, useState } from "react";
import { invariant } from "../fleet/invariant.ts";

const PREFIX = "aether.draft.v1:";
/** Far beyond any message a person types; a longer value in storage is not one of ours. */
export const DRAFT_LIMIT = 200_000;

/** Where a draft belongs: a composer or Steer form (`slot` "message"), or one question's answer. */
export function draftKey(conn: ConnId, agent: string, slot: string): string {
  invariant(conn !== "" && agent !== "", "a draft belongs to a connection and an agent");
  invariant(slot !== "", "a draft names its slot");
  return PREFIX + JSON.stringify([conn, agent, slot]);
}

/** One sessionStorage call; a full or disabled store is an expected failure, returned as its words. */
export function sessionStored<T>(call: (store: Storage) => T): { readonly value: T } | { readonly problem: string } {
  try {
    return { value: call(sessionStorage) };
  } catch (error) {
    if (!(error instanceof DOMException)) throw error;
    return { problem: error.message || error.name };
  }
}

/**
 * The stored draft, "" when there is none or what is stored is not a draft (it is then removed), or
 * null when the store cannot be read.
 */
export function readDraft(key: string): string | null {
  invariant(key.startsWith(PREFIX), "drafts are read by a key from draftKey");
  const raw = sessionStored((store) => store.getItem(key));
  if ("problem" in raw) return null;
  if (raw.value === null) return "";
  const text = (parsedStored(raw.value) as { text?: unknown } | null)?.text;
  if (typeof text === "string" && text.length <= DRAFT_LIMIT) return text;
  // Not a draft of ours: dropping it is best effort, as it is never shown either way.
  sessionStored((store) => store.removeItem(key));
  return "";
}

/** Storage another version or a person edited may hold anything; what does not parse is not ours. */
export function parsedStored(raw: string): unknown {
  try {
    const value: unknown = JSON.parse(raw);
    return typeof value === "object" ? value : null;
  } catch (error) {
    if (!(error instanceof SyntaxError)) throw error;
    return null;
  }
}

/** Stores the draft; why the store could not keep it, in words, or null. */
export function writeDraft(key: string, text: string): string | null {
  invariant(key.startsWith(PREFIX), "drafts are written by a key from draftKey");
  invariant(text.length <= DRAFT_LIMIT, "a draft fits the store's bound");
  const wrote = sessionStored((store) => (text === "" ? store.removeItem(key) : store.setItem(key, JSON.stringify({ text }))));
  return "problem" in wrote ? `This draft is not kept if you leave (${wrote.problem})` : null;
}

export interface Draft {
  readonly text: string;
  /** Why the draft lives only on this screen (the store cannot keep it), or null. */
  readonly problem: string | null;
  readonly set: (text: string) => void;
  /** Clears the draft if it still reads `sent`; text typed since the send is kept. */
  readonly delivered: (sent: string) => void;
}

/**
 * The draft as this screen holds it. `stored` is what it last read or wrote in the store, so a late
 * confirmation can tell its own stale copy from another screen's newer draft; null when unreadable.
 */
interface Held { readonly key: string; readonly text: string; readonly stored: string | null; readonly writeProblem: string | null }

const UNREADABLE = "This draft is not kept if you leave (the store cannot be read)";

function heldFor(key: string): Held {
  const text = readDraft(key);
  return { key, text: text ?? "", stored: text, writeProblem: null };
}

// A store that cannot be read gives nothing back on return, whatever a write did.
const problemOf = (held: Held): string | null => held.writeProblem ?? (held.stored === null ? UNREADABLE : null);

/** A draft that survives unmounting; it follows `key` when the screen switches agent or question. */
export function useDraft(key: string): Draft {
  const [draft, setDraft] = useState(() => heldFor(key));
  const current = draft.key === key ? draft : heldFor(key);
  if (current !== draft) setDraft(current);
  // What this screen last held, read when a send it started is confirmed, however late.
  const typed = useRef<Held | null>(null);
  const latest = useCallback(() => (typed.current?.key === key ? typed.current : current), [key, current]);
  const set = useCallback((text: string) => {
    const base = latest();
    invariant(base.key === key, "a draft is written over the one this screen holds for its key");
    const writeProblem = writeDraft(key, text);
    const next = { key, text, stored: writeProblem === null && base.stored !== null ? text : base.stored, writeProblem };
    typed.current = next;
    setDraft(next);
  }, [key, latest]);
  const delivered = useCallback((sent: string) => {
    const mine = latest();
    invariant(mine.key === key, "a confirmation is checked against this key's draft");
    if (mine.text !== sent) return;
    // The store holding neither what was sent nor what this screen left there was changed by another screen.
    const kept = mine.stored === null ? null : readDraft(key);
    if (kept !== null && kept !== sent && kept !== mine.stored) return;
    set("");
  }, [key, latest, set]);
  return { text: current.text, problem: problemOf(current), set, delivered };
}
