// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  NotificationAction,
  NotificationAnswer,
  NotificationId,
  NotificationKind,
  NotificationState,
} from "../../protocol.ts";

export interface Notification {
  "actions"?: Array<NotificationAction>;
  "answer"?: NotificationAnswer | null;
  "body"?: string;
  "created_at": string;
  "free_text"?: boolean;
  "id": NotificationId;
  "key"?: string | null;
  "kind": NotificationKind;
  "link"?: string | null;
  "source": string;
  "state": NotificationState;
  "title": string;
  "updated_at": string;
}
