// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentConfig,
  AgentId,
  ApprovalPolicy,
  ModelRef,
  ReferenceRequest,
  RunId,
  Scope,
} from "../../protocol.ts";

/**
 * Request to create an agent, optionally with an initial task.
 * A child requires explicit `config.effort`: low, medium, high, xhigh or max.
 */
export interface CreateAgent {
  "approval"?: ApprovalPolicy | null;
  "config"?: AgentConfig | null;
  "fallbacks"?: Array<ModelRef>;
  "idempotency_key"?: string | null;
  "model"?: ModelRef | null;
  "name"?: string | null;
  "objective"?: string | null;
  "parent"?: AgentId | null;
  "references"?: Array<ReferenceRequest>;
  "repository_root"?: string | null;
  "role": string;
  "run"?: RunId | null;
  "scope"?: Scope | null;
  "start"?: boolean;
}
