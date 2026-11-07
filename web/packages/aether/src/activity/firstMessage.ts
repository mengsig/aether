// A new chat's first message that was refused or whose reply never came, kept per connection and
// agent in sessionStorage so the chat it belongs to can say so and send it again, as typed.

import { useApp } from "@aether-ui/core";
import type { ConnId } from "@aether-ui/protocol";
import { aether } from "@aether-ui/protocol";
import { useCallback } from "react";
import { DRAFT_LIMIT, parsedStored, sessionStored } from "../composer/drafts.ts";
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

/**
 * The kept record, or null when there is none or what is stored is not one (it is then removed). A
 * store that cannot be read holds none: keeping one there failed, and the start sheet said so.
 */
export function readUndelivered(key: string): Undelivered | null {
  invariant(key.startsWith(PREFIX), "first messages are read by a key from firstMessageKey");
  const raw = sessionStored((store) => store.getItem(key));
  if ("problem" in raw || raw.value === null) return null;
  const value = parsedStored(raw.value);
  if (isUndelivered(value)) return { text: value.text, outcome: value.outcome, reason: value.reason };
  // An invalid record is never shown, so one the store cannot remove changes nothing.
  forgetUndelivered(key);
  return null;
}

/** Keeps the record for the chat; why the store could not keep it, in words, or null. */
function keepUndelivered(key: string, record: Undelivered): string | null {
  invariant(isUndelivered(record), "only a whole record is kept");
  const kept = sessionStored((store) => store.setItem(key, JSON.stringify(record)));
  return "problem" in kept ? `The chat cannot be told about it (${kept.problem}), so keep this open until it is sent.` : null;
}

/** Removes the record; why the store could not, in words, or null. */
export function forgetUndelivered(key: string): string | null {
  invariant(key.startsWith(PREFIX), "first messages are forgotten by a key from firstMessageKey");
  const forgot = sessionStored((store) => store.removeItem(key));
  return "problem" in forgot ? `The first message's kept copy could not be cleared (${forgot.problem}); the chat may offer it again.` : null;
}

/**
 * Sends a chat's first message as typed (never read as a command) and keeps what became of it for
 * that chat: forgotten once delivered, kept with its reason when refused or unconfirmed.
 */
export function useFirstSend(conn: ConnId): { readonly run: Run; readonly send: (agent: string, text: string) => Promise<FirstSend> } {
  const run = useRun(conn);
  const app = useApp();
  const { attempt } = run;
  const send = useCallback(async (agent: string, text: string): Promise<FirstSend> => {
    invariant(text.trim() === text && text !== "" && text.length <= FIRST_MESSAGE_LIMIT, "a first message is validated before it is sent");
    const key = firstMessageKey(conn, agent);
    const { reply, status } = await attempt(aether.sendMessage({ to: agent, content: text }), "Sent");
    if (reply !== null) {
      // Delivered either way: a copy the store cannot clear is said, never a reason to stall.
      const problem = forgetUndelivered(key);
      if (problem !== null) app.store.report(problem, conn);
      return { outcome: "sent" };
    }
    const record: Undelivered = { text, outcome: status.state === "unconfirmed" ? "unconfirmed" : "refused", reason: status.message };
    return { ...record, keepProblem: keepUndelivered(key, record) };
  }, [app, attempt, conn]);
  return { run, send };
}
