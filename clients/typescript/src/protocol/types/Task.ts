// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Actor,
  AgentId,
  ArtifactId,
  Priority,
  RunId,
  TaskId,
  TaskReference,
  TaskStatus,
  Usage,
  status,
} from "../../protocol.ts";

/**
 * A durable task record.
 */
export interface Task {
  "artifacts"?: Array<ArtifactId>;
  "created_at": string;
  "creator": Actor;
  "delegated_usage"?: Usage | null;
  "delegation_reason"?: string | null;
  "dependencies"?: Array<TaskId>;
  "finished_at"?: string | null;
  "id": TaskId;
  "objective": string;
  "own_usage"?: Usage | null;
  "owner"?: AgentId | null;
  "parent_task"?: TaskId | null;
  "priority"?: Priority;
  "references"?: Array<TaskReference>;
  "run": RunId;
  "started_at"?: string | null;
  "status": TaskStatus;
  "updated_at": string;
}
