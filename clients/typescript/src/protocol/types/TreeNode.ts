// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Agent,
  AgentId,
  Lease,
  Task,
  TaskId,
  Usage,
} from "../../protocol.ts";

/**
 * One node of the live graph.
 */
export interface TreeNode {
  "agent": Agent;
  "children": Array<AgentId>;
  "hidden_usage"?: Usage | null;
  "lease"?: Lease | null;
  "subtree_usage": Usage;
  "task"?: Task | null;
  "waiting_on"?: Array<TaskId>;
}
