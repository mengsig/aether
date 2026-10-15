// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Actor,
  InvocationId,
  MemoryEvidence,
  MemoryFeedbackId,
  MemoryId,
  MemoryVerdict,
  TaskId,
} from "../../protocol.ts";

export interface MemoryFeedback {
  "actor": Actor;
  "created_at": string;
  "evidence": MemoryEvidence;
  "id": MemoryFeedbackId;
  "invocation"?: InvocationId | null;
  "memory": MemoryId;
  "reason": string;
  "retracted_at"?: string | null;
  "retraction_reason"?: string | null;
  "task": TaskId;
  "verdict": MemoryVerdict;
}
