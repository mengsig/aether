// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  ApprovalPolicy,
} from "../../protocol.ts";

/**
 * Turn approvals off (yolo) or back on for an agent and, with
 * `subtree`, every live agent beneath it. A separate command
 * from `UpdateAgent` because it carries the subtree reach and the
 * never-widen-past-the-parent rule.
 */
export interface SetApprovalsCommand {
  "cmd": "set_approvals";
  "agent": AgentId;
  "idempotency_key"?: string | null;
  "policy": ApprovalPolicy;
  "subtree"?: boolean;
}

export function setApprovals(args: Omit<SetApprovalsCommand, "cmd">): SetApprovalsCommand {
  return { cmd: "set_approvals", ...args };
}
