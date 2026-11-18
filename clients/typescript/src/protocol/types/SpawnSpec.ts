// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Agent,
  AgentConfig,
  ApprovalPolicy,
  ModelRef,
  Scope,
} from "../../protocol.ts";

/**
 * Agent + task each spawn occurrence creates: a root in a new run, or its
 * creator's child. Fields as on `create_agent`; no parent or existing run.
 */
export interface SpawnSpec {
  "approval"?: ApprovalPolicy | null;
  "config"?: AgentConfig | null;
  "fallbacks"?: Array<ModelRef>;
  "model"?: ModelRef | null;
  "name"?: string | null;
  "objective": string;
  "role": string;
  "scope"?: Scope | null;
}
