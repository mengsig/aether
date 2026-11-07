// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ConceptRef,
  ConceptStateKind,
} from "../../protocol.ts";

export interface ConceptState {
  "as_of_evidence": number;
  "calibration"?: number | null;
  "concept": ConceptRef;
  "last_success_at"?: string | null;
  "misconceptions": Array<string>;
  "review_due_at"?: string | null;
  "state": ConceptStateKind;
  "strength": number;
}
