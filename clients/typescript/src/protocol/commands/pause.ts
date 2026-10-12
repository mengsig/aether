// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  status,
} from "../../protocol.ts";

/**
 * Pause an agent: a durable flag, orthogonal to its status. With
 * `subtree` it reaches every live agent beneath it too (§3.1).
 */
export interface PauseCommand {
  "cmd": "pause";
  "agent": AgentId;
  "idempotency_key"?: string | null;
  "subtree"?: boolean;
}

export function pause(args: Omit<PauseCommand, "cmd">): PauseCommand {
  return { cmd: "pause", ...args };
}
