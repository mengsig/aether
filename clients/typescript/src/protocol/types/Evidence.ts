// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  EvidenceKind,
  EvidenceOutcome,
  MissClass,
  ProblemMode,
} from "../../protocol.ts";

/**
 * One append-only row of the learner's history.
 */
export interface Evidence {
  "at": string;
  "concept": string;
  "confidence"?: number | null;
  "hint_rung"?: number | null;
  "id": number;
  "kind": EvidenceKind;
  "miss_class"?: MissClass | null;
  "mode"?: ProblemMode | null;
  "note"?: string | null;
  "outcome": EvidenceOutcome;
  "session"?: string | null;
}
