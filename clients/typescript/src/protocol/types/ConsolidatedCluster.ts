// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ClusterOutcomeKind,
  MemoryId,
  MemoryScope,
} from "../../protocol.ts";

export interface ConsolidatedCluster {
  "members": Array<MemoryId>;
  "outcome": ClusterOutcomeKind;
  "promoted"?: boolean;
  "reason"?: string | null;
  "scope": MemoryScope;
  "survivor": MemoryId;
}
