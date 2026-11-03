// Reading the `{reply, data}` objects both daemons answer with.

import { DaemonError } from "./errors.ts";
import { object, seq, type Json } from "./guards.ts";

/** The `data` of a reply, after checking its tag. */
export function replyData(reply: Json, expected: string): unknown {
  if (reply.reply !== expected) throw new DaemonError("internal", `Expected a ${expected} reply, got ${String(reply.reply)}`);
  return reply.data;
}

export function replyObject(reply: Json, expected: string): Json {
  return object(replyData(reply, expected), `${expected} reply`);
}

export function subscriptionId(reply: Json): number {
  return seq(replyObject(reply, "subscribed").id, "subscribed.id");
}

export const RESYNC_CODES: ReadonlySet<string> = new Set(["history_expired", "not_found"]);
