// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  MemoryId,
} from "../../protocol.ts";

/**
 * Reject a pending proposal with an explanation. Needs `memory_proposals`.
 */
export interface RejectMemoryCommand {
  "cmd": "reject_memory";
  "id": MemoryId;
  "reason": string;
}

export function rejectMemory(args: Omit<RejectMemoryCommand, "cmd">): RejectMemoryCommand {
  return { cmd: "reject_memory", ...args };
}
