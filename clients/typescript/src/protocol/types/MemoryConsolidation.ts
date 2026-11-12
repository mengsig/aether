// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ConsolidatedCluster,
} from "../../protocol.ts";

export interface MemoryConsolidation {
  "clusters": Array<ConsolidatedCluster>;
  "dry_run": boolean;
  "scanned": number;
}
