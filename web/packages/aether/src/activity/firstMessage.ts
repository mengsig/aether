// A new chat's first message that was refused or whose reply never came, kept per connection and
// agent in sessionStorage so the chat it belongs to can say so and send it again, as typed.

import type { ConnId } from "@aether-ui/protocol";
import { aether } from "@aether-ui/protocol";
import { useCallback } from "react";
import { DRAFT_LIMIT, parsedStored } from "../composer/drafts.ts";
import { useRun, type Run } from "../controls/kit.tsx";
import { invariant } from "../fleet/invariant.ts";

const PREFIX = "aether.first-message.v1:";
/** The longest first message a chat starts with: it must fit the store that carries it to the chat. */
export const FIRST_MESSAGE_LIMIT = DRAFT_LIMIT;

export interface Undelivered {
  readonly text: string;
  /** `refused`: the daemon said no. `unconfirmed`: the reply never came, so it may have arrived. */
  readonly outcome: "refused" | "unconfirmed";
  readonly reason: string;
}

export type FirstSend =
  | { readonly outcome: "sent" }
  | (Undelivered & { readonly keepProblem: string | null });

export function firstMessageKey(conn: ConnId, agent: string): string {
  invariant(conn !== "" && agent.startsWith("agt_"), "a first message belongs to a connection's agent");
  return PREFIX + JSON.stringify([conn, agent]);
}

const isUndelivered = (value: unknown): value is Undelivered => {
  const v = value as Partial<Undelivered> | null;
  return typeof v?.text === "string" && v.text !== "" && v.text === v.text.trim() && v.text.length <= FIRST_MESSAGE_LIMIT
    && (v.outcome === "refused" || v.outcome === "unconfirmed") && typeof v.reason === "string";
};

/** The kept record, or null when there is none or what is stored is not one (it is then removed). */
export function readUndelivered(key: string): Undelivered | null {
  invariant(key.startsWith(PREFIX), "first messages are read by a key from firstMessageKey");
  const raw = sessionStorage.getItem(key);
  if (raw === null) return null;
  const value = parsedStored(raw);
  if (isUndelivered(value)) return { text: value.text, outcome: value.outcome, reason: value.reason };
  sessionStorage.removeItem(key);
  return null;
}

/** Keeps the record for the chat; a full or disabled store is an expected failure, returned as words. */
function keepUndelivered(key: string, record: Undelivered): string | null {
  invariant(isUndelivered(record), "only a whole record is kept");
  try {
    sessionStorage.setItem(key, JSON.stringify(record));
    return null;
  } catch (error) {
    if (!(error instanceof DOMException)) throw error;
    return `The chat cannot be told about it (${error.message || error.name}), so keep this open until it is sent.`;
  }
}

export function forgetUndelivered(key: string): void {
  invariant(key.startsWith(PREFIX), "first messages are forgotten by a key from firstMessageKey");
  sessionStorage.removeItem(key);
}

/**
 * Sends a chat's first message as typed (never read as a command) and keeps what became of it for
 * that chat: forgotten once delivered, kept with its reason when refused or unconfirmed.
 */
export function useFirstSend(conn: ConnId): { readonly run: Run; readonly send: (agent: string, text: string) => Promise<FirstSend> } {
  const run = useRun(conn);
  const { attempt } = run;
  const send = useCallback(async (agent: string, text: string): Promise<FirstSend> => {
    invariant(text.trim() === text && text !== "" && text.length <= FIRST_MESSAGE_LIMIT, "a first message is validated before it is sent");
    const key = firstMessageKey(conn, agent);
    const { reply, status } = await attempt(aether.sendMessage({ to: agent, content: text }), "Sent");
    if (reply !== null) {
      forgetUndelivered(key);
      return { outcome: "sent" };
    }
    const record: Undelivered = { text, outcome: status.state === "unconfirmed" ? "unconfirmed" : "refused", reason: status.message };
    return { ...record, keepProblem: keepUndelivered(key, record) };
  }, [attempt, conn]);
  return { run, send };
}
