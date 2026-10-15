// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  MemoryId,
} from "../../protocol.ts";


// Commands. Each builds one frame; the client adds the request id.

/**
 * Accept a pending proposal and apply its deferred replacement. Needs `memory_proposals`.
 */
export interface AcceptMemoryCommand {
  "cmd": "accept_memory";
  "id": MemoryId;
  "reason"?: string | null;
}

export function acceptMemory(args: Omit<AcceptMemoryCommand, "cmd">): AcceptMemoryCommand {
  return { cmd: "accept_memory", ...args };
}
