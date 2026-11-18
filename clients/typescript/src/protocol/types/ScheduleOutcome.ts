// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  InboxItemId,
  RunId,
  TaskId,
} from "../../protocol.ts";

/**
 * Tagged by `outcome`.
 */
export interface ScheduleOutcomeDelivered {
  "inbox": InboxItemId;
  "outcome": "delivered";
}

/**
 * A spawn schedule started an agent and its task: a root, or its creator's child.
 */
export interface ScheduleOutcomeSpawned {
  "agent": AgentId;
  "outcome": "spawned";
  "run": RunId;
  "task": TaskId;
}

export interface ScheduleOutcomeSkipped {
  "outcome": "skipped";
  "reason": string;
}

export interface ScheduleOutcomeFailed {
  "error": string;
  "outcome": "failed";
}

export type ScheduleOutcome =
    ScheduleOutcomeDelivered |
    ScheduleOutcomeSpawned |
    ScheduleOutcomeSkipped |
    ScheduleOutcomeFailed
;
