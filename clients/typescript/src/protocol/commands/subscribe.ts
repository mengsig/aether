// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AttentionKind,
  EventQuery,
  PushSubscription,
} from "../../protocol.ts";

/**
 * Stream events with `seq > after` (0 replays everything, `None` starts live).
 */
export interface SubscribeCommand {
  "cmd": "subscribe";
  "after"?: number | null;
  "deltas": boolean;
  "query": EventQuery;
}

export function subscribe(args: Omit<SubscribeCommand, "cmd">): SubscribeCommand {
  return { cmd: "subscribe", ...args };
}

/**
 * Store the calling device's push subscription, replacing any earlier
 * one. Only a paired device can call it.
 */
export interface SubscribePushCommand {
  "cmd": "subscribe_push";
  "kinds"?: Array<AttentionKind>;
  "subscription": PushSubscription;
}

export function subscribePush(args: Omit<SubscribePushCommand, "cmd">): SubscribePushCommand {
  return { cmd: "subscribe_push", ...args };
}
