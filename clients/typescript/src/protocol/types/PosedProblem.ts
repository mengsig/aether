// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Problem,
  ProblemConclusion,
} from "../../protocol.ts";

/**
 * A posed problem with its conclusion once it has one.
 */
export interface PosedProblem {
  "conclusion"?: ProblemConclusion | null;
  "problem": Problem;
}
