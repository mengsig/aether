// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  MemoryId,
  MemoryKind,
  MemoryProvenance,
  MemoryScope,
  Retraction,
} from "../../protocol.ts";

/**
 * A stored memory.
 */
export interface MemoryRecord {
  "accepted_at"?: string | null;
  "confidence": number;
  "created_at": string;
  "forgotten_at"?: string | null;
  "id": MemoryId;
  "importance": number;
  "kind": MemoryKind;
  "last_retrieved_at"?: string | null;
  "pinned"?: boolean;
  "proposed_at"?: string | null;
  "provenance": MemoryProvenance;
  "retracted"?: Retraction | null;
  "retrieval_count"?: number;
  "scope": MemoryScope;
  "structured"?: unknown;
  "superseded_by"?: MemoryId | null;
  "supersedes"?: MemoryId | null;
  "tags"?: Array<string>;
  "text": string;
}
