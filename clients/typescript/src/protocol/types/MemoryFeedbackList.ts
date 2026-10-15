// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  FeedbackRanking,
  MemoryFeedback,
} from "../../protocol.ts";

export interface MemoryFeedbackList {
  "ranking"?: FeedbackRanking;
  "records": Array<MemoryFeedback>;
  "truncated": boolean;
}
