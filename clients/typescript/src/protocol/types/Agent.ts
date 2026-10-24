// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentConfig,
  AgentId,
  AgentStats,
  AgentStatus,
  CheckpointId,
  EngineSentLayers,
  EngineSessionUsage,
  ModelRef,
  Role,
  RunId,
  Scope,
  TaskId,
  WaitCondition,
  status,
} from "../../protocol.ts";

/**
 * A durable agent record. This is authoritative state; the model is never
 * asked to remember any of it.
 */
export interface Agent {
  "archived"?: boolean;
  "checkpoint"?: CheckpointId | null;
  "config": AgentConfig;
  "created_at": string;
  "current_task"?: TaskId | null;
  "engine_alive"?: boolean;
  "engine_sent"?: EngineSentLayers | null;
  "engine_session"?: string | null;
  "engine_usage"?: EngineSessionUsage | null;
  "fallbacks"?: Array<ModelRef>;
  "id": AgentId;
  "inbox_cursor"?: number;
  "last_event_at"?: string | null;
  "model": ModelRef;
  "name": string;
  "parent"?: AgentId | null;
  "paused"?: boolean;
  "paused_via"?: AgentId | null;
  "restarted_as"?: AgentId | null;
  "restarted_from"?: AgentId | null;
  "resume_wait"?: WaitCondition | null;
  "role": Role;
  "run": RunId;
  "scope": Scope;
  "stats"?: AgentStats;
  "status": AgentStatus;
  "status_since"?: string | null;
  "updated_at": string;
}
