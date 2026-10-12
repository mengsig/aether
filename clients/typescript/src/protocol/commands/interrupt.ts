// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
} from "../../protocol.ts";

/**
 * Stop the running turn only; the agent keeps its transcript and waits
 * for the next message.
 */
export interface InterruptCommand {
  "cmd": "interrupt";
  "agent": AgentId;
  "idempotency_key"?: string | null;
}

export function interrupt(args: Omit<InterruptCommand, "cmd">): InterruptCommand {
  return { cmd: "interrupt", ...args };
}
