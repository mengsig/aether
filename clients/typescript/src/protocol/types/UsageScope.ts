// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  RunId,
} from "../../protocol.ts";

/**
 * Which invocations `usage_breakdown` sums. Every field given narrows the
 * set; none sums the whole fleet.
 */
export interface UsageScope {
  "agent"?: AgentId | null;
  "role_prefix"?: string | null;
  "run"?: RunId | null;
  "subtree"?: AgentId | null;
}
