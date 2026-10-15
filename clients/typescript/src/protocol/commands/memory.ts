// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  MemoryId,
} from "../../protocol.ts";

/**
 * Newest evidence first, including retractions. Needs `memory_feedback`.
 */
export interface MemoryFeedbackCommand {
  "cmd": "memory_feedback";
  "limit"?: number | null;
  "memory": MemoryId;
}

export function memoryFeedback(args: Omit<MemoryFeedbackCommand, "cmd">): MemoryFeedbackCommand {
  return { cmd: "memory_feedback", ...args };
}
