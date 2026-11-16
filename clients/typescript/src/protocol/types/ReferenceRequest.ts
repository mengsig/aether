// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ReferenceKind,
} from "../../protocol.ts";

/**
 * One reference as a creator names it: `{path, kind, label}`.
 */
export interface ReferenceRequest {
  "kind": ReferenceKind;
  "label"?: string | null;
  "path": string;
}
