// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Actor,
} from "../../protocol.ts";

/**
 * The operator's answer: a chosen action, their own words, or both.
 */
export interface NotificationAnswer {
  "action"?: string | null;
  "at": string;
  "by": Actor;
  "text"?: string | null;
}
