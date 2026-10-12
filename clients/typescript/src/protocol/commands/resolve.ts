// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ApprovalDecision,
  ApprovalId,
  RememberSpec,
} from "../../protocol.ts";


export interface ResolveApprovalCommand {
  "cmd": "resolve_approval";
  "decision": ApprovalDecision;
  "id": ApprovalId;
  "idempotency_key"?: string | null;
  "note"?: string | null;
  "remember"?: RememberSpec | null;
}

export function resolveApproval(args: Omit<ResolveApprovalCommand, "cmd">): ResolveApprovalCommand {
  return { cmd: "resolve_approval", ...args };
}

/**
 * Ask what became of a keyed command whose acknowledgement was lost.
 * Read-only, within the caller's own keys; needs `command`'s standing.
 */
export interface ResolveSubmissionCommand {
  "cmd": "resolve_submission";
  "command": Record<string, unknown>;
  "horizon"?: string | null;
}

export function resolveSubmission(args: Omit<ResolveSubmissionCommand, "cmd">): ResolveSubmissionCommand {
  return { cmd: "resolve_submission", ...args };
}
