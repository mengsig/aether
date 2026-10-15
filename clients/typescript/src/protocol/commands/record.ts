// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  InvocationId,
  MemoryEvidence,
  MemoryId,
  MemoryVerdict,
  TaskId,
} from "../../protocol.ts";

/**
 * Record host evidence on a memory a task saw. Needs `memory_feedback`.
 */
export interface RecordMemoryFeedbackCommand {
  "cmd": "record_memory_feedback";
  "evidence": MemoryEvidence;
  "idempotency_key"?: string | null;
  "invocation"?: InvocationId | null;
  "memory": MemoryId;
  "reason": string;
  "task": TaskId;
  "verdict": MemoryVerdict;
}

export function recordMemoryFeedback(args: Omit<RecordMemoryFeedbackCommand, "cmd">): RecordMemoryFeedbackCommand {
  return { cmd: "record_memory_feedback", ...args };
}
