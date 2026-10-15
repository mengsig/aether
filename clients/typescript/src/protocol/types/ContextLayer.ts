// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  LayerKind,
  LayerPlacement,
  MemoryId,
  memories,
} from "../../protocol.ts";

/**
 * One rendered layer. System layers carry text; the tail carries messages
 * and the tool layer carries schemas, so those keep only counts here.
 */
export interface ContextLayer {
  "items"?: number;
  "kind": LayerKind;
  "memories"?: Array<MemoryId>;
  "placement"?: LayerPlacement;
  "text"?: string | null;
  "tokens": number;
}
