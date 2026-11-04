// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ConceptKind,
} from "../../protocol.ts";

export interface ConceptRef {
  "id": string;
  "kind": ConceptKind;
  "ref": string;
  "title": string;
}
