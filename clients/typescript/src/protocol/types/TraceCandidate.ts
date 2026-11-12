// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  MemoryId,
  TraceOutcome,
} from "../../protocol.ts";

export interface TraceCandidate {
  "memory": MemoryId;
  "outcome": TraceOutcome;
  "pinned": boolean;
  "rank"?: number | null;
  "relevance": number;
  "score"?: number | null;
  "stale": boolean;
}
