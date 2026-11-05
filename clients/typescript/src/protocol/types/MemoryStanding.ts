// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  MemoryRef,
  MemoryState,
  MemorySupport,
  MemoryTier,
  StaleNote,
  Verification,
} from "../../protocol.ts";

/**
 * What admission and retrieval know about a record, sent beside it to a
 * client that lists `memory_v2`. Never part of the stored body.
 */
export interface MemoryStanding {
  "refs"?: Array<MemoryRef>;
  "stale"?: StaleNote | null;
  "state": MemoryState;
  "support": MemorySupport;
  "tier": MemoryTier;
  "verification": Verification;
}
