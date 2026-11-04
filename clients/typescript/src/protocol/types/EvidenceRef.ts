// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  EvidenceRefKind,
} from "../../protocol.ts";

export interface EvidenceRef {
  "id": string;
  "kind": EvidenceRefKind;
  "url"?: string | null;
}
