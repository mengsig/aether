// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  EngineeringExtraction,
  SessionExtraction,
} from "../../protocol.ts";

export interface ExtractionPatch {
  "engineering"?: EngineeringExtraction | null;
  "sessions"?: SessionExtraction | null;
}
