// Unsent text, kept per connection and agent (and per question) in sessionStorage, so leaving a
// screen does not lose it. A draft is cleared only once its delivery is confirmed.

import type { ConnId } from "@aether-ui/protocol";
import { useCallback, useState } from "react";
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

/** The stored draft, or "" when there is none or what is stored is not a draft (it is then removed). */
export function readDraft(key: string): string {
  invariant(key.startsWith(PREFIX), "drafts are read by a key from draftKey");
  const raw = sessionStorage.getItem(key);
  if (raw === null) return "";
  const text = (parsedStored(raw) as { text?: unknown } | null)?.text;
  if (typeof text === "string" && text.length <= DRAFT_LIMIT) return text;
  sessionStorage.removeItem(key);
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

export function writeDraft(key: string, text: string): void {
  invariant(key.startsWith(PREFIX), "drafts are written by a key from draftKey");
  invariant(text.length <= DRAFT_LIMIT, "a draft fits the store's bound");
  if (text === "") sessionStorage.removeItem(key);
  else sessionStorage.setItem(key, JSON.stringify({ text }));
}

export interface Draft {
  readonly text: string;
  readonly set: (text: string) => void;
  /** Clears the draft if it still reads `sent`; text typed since the send is kept. */
  readonly delivered: (sent: string) => void;
}

/** A draft that survives unmounting; it follows `key` when the screen switches agent or question. */
export function useDraft(key: string): Draft {
  const [draft, setDraft] = useState(() => ({ key, text: readDraft(key) }));
  const current = draft.key === key ? draft : { key, text: readDraft(key) };
  if (current !== draft) setDraft(current);
  const set = useCallback((text: string) => {
    writeDraft(key, text);
    setDraft({ key, text });
  }, [key]);
  const delivered = useCallback((sent: string) => {
    if (readDraft(key) !== sent) return;
    writeDraft(key, "");
    setDraft({ key, text: "" });
  }, [key]);
  return { text: current.text, set, delivered };
}
