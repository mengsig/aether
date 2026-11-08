// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  MissClass,
  PracticeOutcome,
} from "../../protocol.ts";

/**
 * How a problem ended, as `noesis.practice_concluded` announced it.
 */
export interface ProblemConclusion {
  "concluded_at": string;
  "miss_class"?: MissClass | null;
  "outcome": PracticeOutcome;
  "retest": boolean;
  "rung"?: number | null;
  "solution": string;
}
