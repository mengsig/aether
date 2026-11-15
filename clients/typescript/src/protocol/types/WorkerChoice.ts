// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Effort,
  ModelRef,
  ModelTier,
} from "../../protocol.ts";

/**
 * One allowed kind of child: an exact model or a whole tier, optionally
 * capped in effort. Exactly one of `model` and `tier` is set.
 */
export interface WorkerChoice {
  "effort"?: Effort | null;
  "model"?: ModelRef | null;
  "tier"?: ModelTier | null;
}
