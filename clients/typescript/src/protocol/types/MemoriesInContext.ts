// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  SnapshotMemories,
  memories,
} from "../../protocol.ts";

/**
 * Which memories each model call of a task, or one invocation, was given,
 * oldest snapshot first. `truncated` says snapshots past the limit were left out.
 */
export interface MemoriesInContext {
  "snapshots": Array<SnapshotMemories>;
  "truncated"?: boolean;
}
