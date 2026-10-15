// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  MemoryId,
  MemoryKind,
  OperatorMemoryScope,
} from "../../protocol.ts";

/**
 * A person supplies content and applicability, never fabricated provenance.
 */
export interface OperatorMemorySpec {
  "confidence"?: number | null;
  "importance"?: number | null;
  "kind": MemoryKind;
  "proposed"?: boolean;
  "scope": OperatorMemoryScope;
  "supersedes"?: MemoryId | null;
  "tags"?: Array<string>;
  "text": string;
}
