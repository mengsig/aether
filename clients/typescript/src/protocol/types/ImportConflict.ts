// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  MemoryId,
} from "../../protocol.ts";

export interface ImportConflict {
  "memory": MemoryId;
  "reason": string;
}
