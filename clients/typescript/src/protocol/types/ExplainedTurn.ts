// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  InvocationId,
  MemoryId,
  SnapshotId,
  TaskId,
  TraceCandidate,
  TraceOmitted,
  TracePolicy,
} from "../../protocol.ts";

export interface ExplainedTurn {
  "agent": AgentId;
  "at": string;
  "candidates"?: Array<TraceCandidate>;
  "injected"?: Array<MemoryId>;
  "invocations"?: Array<InvocationId>;
  "omitted"?: TraceOmitted;
  "policy"?: TracePolicy | null;
  "pool": number;
  "snapshot": SnapshotId;
  "task"?: TaskId | null;
  "traced": boolean;
  "turn": number;
}
