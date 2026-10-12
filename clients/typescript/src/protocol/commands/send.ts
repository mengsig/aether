// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  ImageAttachment,
} from "../../protocol.ts";

/**
 * Deliver a user message to an agent's inbox; wakes it if waiting.
 * With `subtree`, the message is fanned out to every live agent beneath
 * it too, each with its own inbox item and events (§3.5).
 */
export interface SendMessageCommand {
  "cmd": "send_message";
  "attachments"?: Array<ImageAttachment>;
  "content": string;
  "idempotency_key"?: string | null;
  "subtree"?: boolean;
  "to": AgentId;
}

export function sendMessage(args: Omit<SendMessageCommand, "cmd">): SendMessageCommand {
  return { cmd: "send_message", ...args };
}
