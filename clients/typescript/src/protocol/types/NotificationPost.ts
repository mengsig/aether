// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  NotificationAction,
  NotificationKind,
} from "../../protocol.ts";

/**
 * What a poster sends. Posting the same `key` again from the same source
 * updates that notification instead of adding another.
 */
export interface NotificationPost {
  "actions"?: Array<NotificationAction>;
  "body"?: string;
  "free_text"?: boolean;
  "key"?: string | null;
  "kind": NotificationKind;
  "link"?: string | null;
  "title": string;
}
