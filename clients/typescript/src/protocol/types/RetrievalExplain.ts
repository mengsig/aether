// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ExplainedTurn,
} from "../../protocol.ts";

/**
 * What retrieval did for each model call of a task, or one invocation.
 */
export interface RetrievalExplain {
  "truncated"?: boolean;
  "turns": Array<ExplainedTurn>;
}
