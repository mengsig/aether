// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  EvidenceItem,
} from "../../protocol.ts";

/**
 * What the frontend gathered from reads the user can already make, stored
 * with the session that starts from it.
 */
export interface EvidenceBundle {
  "items": Array<EvidenceItem>;
  "title": string;
}
