// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  NotificationId,
} from "../../protocol.ts";

/**
 * Answer a notification with one of its actions, free text where it
 * takes it, or both; this resolves it and reaches a posting agent's
 * inbox. Needs `features::NOTIFICATIONS`.
 */
export interface AnswerNotificationCommand {
  "cmd": "answer_notification";
  "action"?: string | null;
  "id": NotificationId;
  "text"?: string | null;
}

export function answerNotification(args: Omit<AnswerNotificationCommand, "cmd">): AnswerNotificationCommand {
  return { cmd: "answer_notification", ...args };
}
