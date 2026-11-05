// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  TraceCandidate,
  TraceOmitted,
  TracePolicy,
} from "../../protocol.ts";

/**
 * What retrieval did for one turn, recorded with the snapshot it built.
 */
export interface RetrievalTrace {
  "candidates": Array<TraceCandidate>;
  "omitted"?: TraceOmitted;
  "policy": TracePolicy;
  "pool": number;
}
