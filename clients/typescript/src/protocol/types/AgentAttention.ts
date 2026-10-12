// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  AttentionKind,
} from "../../protocol.ts";

/**
 * A failed agent, or one blocked on something only the operator can clear.
 */
export interface AgentAttention {
  "agent": AgentId;
  "agent_name"?: string | null;
  "kind": AttentionKind;
  "reason": string;
  "since": string;
}
