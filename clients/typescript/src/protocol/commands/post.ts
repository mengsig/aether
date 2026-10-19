// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  NotificationPost,
} from "../../protocol.ts";

/**
 * Post a notification as the service `source`, or update the one its
 * `key` already names. Needs `features::NOTIFICATIONS`.
 */
export interface PostNotificationCommand {
  "cmd": "post_notification";
  "notification": NotificationPost;
  "source": string;
}

export function postNotification(args: Omit<PostNotificationCommand, "cmd">): PostNotificationCommand {
  return { cmd: "post_notification", ...args };
}
