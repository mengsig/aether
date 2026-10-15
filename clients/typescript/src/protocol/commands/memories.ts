// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  InvocationId,
  MemoryQuerySpec,
  TaskId,
} from "../../protocol.ts";


export interface MemoriesCommand {
  "cmd": "memories";
  "query": MemoryQuerySpec;
}

export function memories(args: Omit<MemoriesCommand, "cmd">): MemoriesCommand {
  return { cmd: "memories", ...args };
}

/**
 * Which stored memories every model call of a task, or one invocation,
 * was given. Name exactly one. Needs `memory_context`.
 */
export interface MemoriesInContextCommand {
  "cmd": "memories_in_context";
  "invocation"?: InvocationId | null;
  "task"?: TaskId | null;
}

export function memoriesInContext(args: Omit<MemoriesInContextCommand, "cmd">): MemoriesInContextCommand {
  return { cmd: "memories_in_context", ...args };
}
