// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  EvidenceRef,
  EvidenceStatus,
  status,
} from "../../protocol.ts";

export interface EvidenceItem {
  "excerpt": string;
  "ref": EvidenceRef;
  "status": EvidenceStatus;
}
