// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ApprovalPolicy,
  Effort,
  HostContext,
  Priority,
  ToolExposure,
} from "../../protocol.ts";

/**
 * Per-agent configuration the runtime enforces.
 */
export interface AgentConfig {
  "approval"?: ApprovalPolicy;
  "build_cache"?: string | null;
  "concurrency"?: number | null;
  "cost_budget"?: number | null;
  "deadline"?: string | null;
  "effort"?: Effort | null;
  "host_context"?: HostContext;
  "instructions"?: string | null;
  "leaf"?: boolean;
  "long_lived"?: boolean;
  "max_output_tokens"?: number | null;
  "max_turns"?: number | null;
  "priority"?: Priority;
  "token_budget"?: number | null;
  "tools"?: ToolExposure;
}
