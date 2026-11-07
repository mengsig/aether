// Unsent text, kept in memory per connection, agent and target (the composer, Steer, one question's
// answer) so leaving a screen does not lose it, and saved to sessionStorage, best effort, for a reload.

import type { ConnId } from "@aether-ui/protocol";
import { useCallback, useSyncExternalStore } from "react";
import { invariant } from "../fleet/invariant.ts";

const PREFIX = "aether.draft.v1:";
/** Far beyond any message a person types; a longer value in storage is not one of ours. */
export const DRAFT_LIMIT = 200_000;

/** Where a draft belongs: `target` is "message" (the composer), "steer" or `question:<seq>`. */
export function draftKey(conn: ConnId, agent: string, target: string): string {
  invariant(conn !== "" && agent !== "", "a draft belongs to a connection and an agent");
  invariant(target !== "", "a draft names its target");
  return PREFIX + JSON.stringify([conn, agent, target]);
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

/** The session's drafts: memory is what screens show; storage only seeds a key after a reload. */
const drafts = new Map<string, string>();
const listeners = new Set<() => void>();
/** Why sessionStorage failed this session, once it has; drafts then live only until a reload. */
let storageProblem: string | null = null;

const notify = () => { for (const listener of listeners) listener(); };
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
};

/** The first storage failure of the session, in words; later ones say nothing new. */
function noteProblem(problem: string): void {
  if (storageProblem !== null) return;
  storageProblem = `Drafts are kept while the app is open, but not across a reload (${problem}).`;
}

/** What sessionStorage holds for `key` from before a reload; "" for nothing, or nothing of ours. */
function restored(key: string): string {
  const raw = sessionStored((store) => store.getItem(key));
  if ("problem" in raw) {
    // Read while rendering, so no listener is called: this render reads the problem next.
    noteProblem(raw.problem);
    return "";
  }
  const text = raw.value === null ? undefined : (parsedStored(raw.value) as { text?: unknown } | null)?.text;
  return typeof text === "string" && text.length <= DRAFT_LIMIT ? text : "";
}

function draftText(key: string): string {
  let text = drafts.get(key);
  if (text === undefined) {
    text = restored(key);
    drafts.set(key, text);
  }
  return text;
}

function setDraft(key: string, text: string): void {
  invariant(key.startsWith(PREFIX), "drafts are written by a key from draftKey");
  invariant(text.length <= DRAFT_LIMIT, "a draft fits the store's bound");
  drafts.set(key, text);
  const wrote = sessionStored((store) => (text === "" ? store.removeItem(key) : store.setItem(key, JSON.stringify({ text }))));
  if ("problem" in wrote) noteProblem(wrote.problem);
  notify();
}

/** Forgets every draft in memory, as a new session starts with none; storage is left as it is. */
export function forgetDrafts(): void {
  drafts.clear();
  storageProblem = null;
  notify();
}

export interface Draft {
  readonly text: string;
  /** Why drafts will not survive a reload (sessionStorage failed), or null. */
  readonly problem: string | null;
  readonly set: (text: string) => void;
  /** Clears the draft only while it is still exactly `sent`; text typed since is kept. */
  readonly delivered: (sent: string) => void;
}

/** The draft for `key`, shared by every screen showing it and kept after they unmount. */
export function useDraft(key: string): Draft {
  invariant(key.startsWith(PREFIX), "drafts are read by a key from draftKey");
  const text = useSyncExternalStore(subscribe, () => draftText(key));
  const problem = useSyncExternalStore(subscribe, () => storageProblem);
  const set = useCallback((next: string) => setDraft(key, next), [key]);
  const delivered = useCallback((sent: string) => {
    if (draftText(key) === sent) setDraft(key, "");
  }, [key]);
  return { text, problem, set, delivered };
}
