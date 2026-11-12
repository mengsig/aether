// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  MemoryScope,
  memories,
} from "../../protocol.ts";

/**
 * Merge near-duplicate memories, or with `dry_run` only plan it.
 * An absent scope means every scope. Needs `memory_v2`.
 */
export interface ConsolidateMemoriesCommand {
  "cmd": "consolidate_memories";
  "dry_run": boolean;
  "scope"?: MemoryScope | null;
}

export function consolidateMemories(args: Omit<ConsolidateMemoriesCommand, "cmd">): ConsolidateMemoriesCommand {
  return { cmd: "consolidate_memories", ...args };
}
