// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
} from "../../protocol.ts";

/**
 * Recreate a failed or cancelled agent from its last checkpoint, as a
 * new agent linked to it.
 */
export interface RestartCommand {
  "cmd": "restart";
  "agent": AgentId;
  "idempotency_key"?: string | null;
}

export function restart(args: Omit<RestartCommand, "cmd">): RestartCommand {
  return { cmd: "restart", ...args };
}
