// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ApprovalDecision,
  RuleMatch,
  RuleScope,
} from "../../protocol.ts";


export interface AddApprovalRuleCommand {
  "cmd": "add_approval_rule";
  "decision": ApprovalDecision;
  "idempotency_key"?: string | null;
  "match": RuleMatch;
  "note"?: string | null;
  "scope": RuleScope;
}

export function addApprovalRule(args: Omit<AddApprovalRuleCommand, "cmd">): AddApprovalRuleCommand {
  return { cmd: "add_approval_rule", ...args };
}
