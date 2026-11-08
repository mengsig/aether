// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  SessionRole,
} from "../../protocol.ts";

/**
 * One agent of a session and the inputs Noesis wrote to it.
 */
export interface SessionAgent {
  "agent": AgentId;
  "internal"?: Array<number>;
  "role": SessionRole;
}
