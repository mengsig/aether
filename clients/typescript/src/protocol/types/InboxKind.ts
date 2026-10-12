// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Actor,
  AgentId,
  ApprovalDecision,
  ApprovalId,
  ContentBlock,
  MessageId,
  TaskId,
  TaskResult,
  wake,
} from "../../protocol.ts";

/**
 * Tagged by `kind`.
 */
export interface InboxKindTaskCompleted {
  "kind": "task_completed";
  "result": TaskResult;
  "task": TaskId;
}

export interface InboxKindTaskFailed {
  "error": string;
  "kind": "task_failed";
  "structured"?: unknown;
  "task": TaskId;
}

export interface InboxKindTaskCancelled {
  "kind": "task_cancelled";
  "reason"?: string | null;
  "task": TaskId;
}

export interface InboxKindMessage {
  "content": Array<ContentBlock>;
  "from": Actor;
  "kind": "message";
  "message_id": MessageId;
}

export interface InboxKindApprovalResolved {
  "approval": ApprovalId;
  "by"?: Actor | null;
  "decision": ApprovalDecision;
  "kind": "approval_resolved";
  "note"?: string | null;
}

export interface InboxKindChildFailed {
  "agent": AgentId;
  "error": string;
  "kind": "child_failed";
}

/**
 * A descendant parked on provider exhaustion; it probes and resumes by
 * itself, so the recipient is informed rather than asked to act.
 */
export interface InboxKindChildSuspended {
  "agent": AgentId;
  "detail": string;
  "kind": "child_suspended";
}

/**
 * The same descendant's provider answered again and its turn continues;
 * informational, so it never interrupts a wait.
 */
export interface InboxKindChildResumed {
  "agent": AgentId;
  "kind": "child_resumed";
}

export interface InboxKindExternalTrigger {
  "kind": "external_trigger";
  "operator"?: boolean;
  "payload": unknown;
  "source": string;
}

/**
 * A bare wake with a reason (deadline passed, operator nudge).
 */
export interface InboxKindWake {
  "kind": "wake";
  "reason": string;
}

export type InboxKind =
    InboxKindTaskCompleted |
    InboxKindTaskFailed |
    InboxKindTaskCancelled |
    InboxKindMessage |
    InboxKindApprovalResolved |
    InboxKindChildFailed |
    InboxKindChildSuspended |
    InboxKindChildResumed |
    InboxKindExternalTrigger |
    InboxKindWake
;
