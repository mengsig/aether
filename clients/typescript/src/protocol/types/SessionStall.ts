// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  StallReason,
} from "../../protocol.ts";

/**
 * A turn that stopped before completion, and why.
 */
export interface SessionStall {
  "at": string;
  "message": string;
  "reason": StallReason;
}
