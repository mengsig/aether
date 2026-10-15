// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  InvocationId,
  MemoryId,
  SnapshotId,
  TaskId,
  memories,
} from "../../protocol.ts";

/**
 * The memories one snapshot put in front of the model, as a host asks for
 * them through `memories_in_context`.
 */
export interface SnapshotMemories {
  "agent": AgentId;
  "at": string;
  "invocations"?: Array<InvocationId>;
  "memories"?: Array<MemoryId>;
  "snapshot": SnapshotId;
  "task"?: TaskId | null;
  "turn": number;
}
