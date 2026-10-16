// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
} from "../../protocol.ts";

/**
 * Bring a failed agent back in place, with its history and context:
 * it becomes Ready and reachable again under the same id. Additive in
 * protocol 8; older daemons refuse the tag as unknown.
 */
export interface WakeCommand {
  "cmd": "wake";
  "agent": AgentId;
}

export function wake(args: Omit<WakeCommand, "cmd">): WakeCommand {
  return { cmd: "wake", ...args };
}
