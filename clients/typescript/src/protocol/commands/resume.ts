// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  pause,
} from "../../protocol.ts";

/**
 * Lift a pause, or clear a wait or an inspection the operator owns.
 * With `subtree` it lifts the pause across the subtree, and only that.
 */
export interface ResumeCommand {
  "cmd": "resume";
  "agent": AgentId;
  "idempotency_key"?: string | null;
  "subtree"?: boolean;
}

export function resume(args: Omit<ResumeCommand, "cmd">): ResumeCommand {
  return { cmd: "resume", ...args };
}
