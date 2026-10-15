// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  MemoryFeedbackId,
} from "../../protocol.ts";

/**
 * Withdraw evidence, preserving the record. Needs `memory_feedback`.
 */
export interface RetractMemoryFeedbackCommand {
  "cmd": "retract_memory_feedback";
  "feedback": MemoryFeedbackId;
  "reason": string;
}

export function retractMemoryFeedback(args: Omit<RetractMemoryFeedbackCommand, "cmd">): RetractMemoryFeedbackCommand {
  return { cmd: "retract_memory_feedback", ...args };
}
