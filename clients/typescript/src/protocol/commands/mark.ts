// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  NotificationId,
} from "../../protocol.ts";

/**
 * Mark a notification seen. Needs `features::NOTIFICATIONS`.
 */
export interface MarkNotificationReadCommand {
  "cmd": "mark_notification_read";
  "id": NotificationId;
}

export function markNotificationRead(args: Omit<MarkNotificationReadCommand, "cmd">): MarkNotificationReadCommand {
  return { cmd: "mark_notification_read", ...args };
}
