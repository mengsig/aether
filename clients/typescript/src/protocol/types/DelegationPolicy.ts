// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Effort,
  ModelRef,
  ModelTier,
  WorkerChoice,
} from "../../protocol.ts";

/**
 * The children a task's agent may create. Every field is optional; an empty
 * policy bounds nothing beyond the rules for a task without one.
 */
export interface DelegationPolicy {
  "default_tier"?: ModelTier | null;
  "exempt_models"?: Array<ModelRef>;
  "exempt_roles"?: Array<string>;
  "max_effort"?: Effort | null;
  "max_parallel"?: number | null;
  "workers"?: Array<WorkerChoice>;
}
