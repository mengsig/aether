// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ApprovalDecision,
  RuleMatch,
  RuleScope,
} from "../../protocol.ts";

/**
 * A remembered decision written directly, without answering a call first.
 */
export interface NewApprovalRule {
  "decision": ApprovalDecision;
  "idempotency_key"?: string | null;
  "match": RuleMatch;
  "note"?: string | null;
  "scope": RuleScope;
}
