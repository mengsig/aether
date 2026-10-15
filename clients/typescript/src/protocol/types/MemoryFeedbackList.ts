// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  MemoryFeedback,
} from "../../protocol.ts";

export interface MemoryFeedbackList {
  "records": Array<MemoryFeedback>;
  "truncated": boolean;
}
