// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ProblemMode,
} from "../../protocol.ts";

/**
 * A practice problem. Its solution is revealed only by
 * `noesis.practice_concluded`.
 */
export interface Problem {
  "id": string;
  "mode": ProblemMode;
  "posed_at": string;
  "statement": string;
  "targets": Array<string>;
}
