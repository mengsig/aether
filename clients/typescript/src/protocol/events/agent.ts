// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentConfig,
  AgentId,
  AgentStatusKind,
  Billing,
  CheckpointId,
  ContentBlock,
  InvocationId,
  MessageRole,
  ModelRef,
  Role,
  Scope,
  TaskId,
  Usage,
  WaitCondition,
  replay,
  status,
} from "../../protocol.ts";

/**
 * Typed event payloads, grouped by family. The serialized `type` tag uses
 * dotted names (`agent.created`) so filters can match by family prefix.
 * Tagged by `type`.
 */
export interface EventKindAgentCreated {
  "fallbacks"?: Array<ModelRef>;
  "model": ModelRef;
  "name": string;
  "parent"?: AgentId | null;
  "role": Role;
  "scope": Scope;
  "type": "agent.created";
}

export interface EventKindAgentReady {
  "type": "agent.ready";
}

export interface EventKindAgentStarted {
  "turn": number;
  "type": "agent.started";
}

export interface EventKindAgentTurnCompleted {
  "invocation"?: InvocationId | null;
  "next": AgentStatusKind;
  "turn": number;
  "type": "agent.turn_completed";
}

/**
 * The operator stopped the running turn. The agent keeps its
 * transcript and waits for the next message.
 */
export interface EventKindAgentInterrupted {
  "turn": number;
  "type": "agent.interrupted";
}

export interface EventKindAgentWaiting {
  "condition": WaitCondition;
  "type": "agent.waiting";
}

export interface EventKindAgentResumed {
  "reason": string;
  "type": "agent.resumed";
  "via"?: AgentId | null;
}

/**
 * The operator paused the agent (VISION §3.1). Durable and orthogonal to
 * the status: it takes no turns until resumed, and its inbox fills.
 */
export interface EventKindAgentPaused {
  "type": "agent.paused";
  "via"?: AgentId | null;
}

/**
 * The agent was cleared out of the live view, or brought back.
 * A view mark: nothing is removed, and replay is unaffected.
 */
export interface EventKindAgentArchived {
  "archived": boolean;
  "type": "agent.archived";
}

export interface EventKindAgentBlocked {
  "dependency"?: TaskId | null;
  "reason": string;
  "type": "agent.blocked";
}

export interface EventKindAgentCompleted {
  "summary"?: string | null;
  "type": "agent.completed";
}

export interface EventKindAgentFailed {
  "error": string;
  "type": "agent.failed";
}

/**
 * A failed or cancelled agent was recreated from its last checkpoint
 * (§3.8). Recorded on both agents so either end of the link is visible
 * from the other's event stream.
 */
export interface EventKindAgentRestarted {
  "checkpoint"?: CheckpointId | null;
  "dropped_deadline"?: string | null;
  "from": AgentId;
  "inbox_carried"?: number;
  "to": AgentId;
  "transcript_dropped"?: number;
  "transcript_tail": number;
  "type": "agent.restarted";
}

export interface EventKindAgentCancelled {
  "reason"?: string | null;
  "type": "agent.cancelled";
}

/**
 * An operator or orchestrator changed the agent's configuration mid-run
 * (model, effort, tool exposure, approval policy, budget).
 */
export interface EventKindAgentConfigUpdated {
  "config": AgentConfig;
  "model"?: ModelRef | null;
  "name"?: string | null;
  "scope"?: Scope | null;
  "type": "agent.config_updated";
}

/**
 * Absolute billing totals, including corrections from a completed engine run.
 */
export interface EventKindAgentUsageUpdated {
  "billing"?: Billing | null;
  "total": Usage;
  "turn_usage": Usage;
  "type": "agent.usage_updated";
}

/**
 * A transcript entry: what the model said, or what it was shown.
 */
export interface EventKindAgentMessage {
  "content": Array<ContentBlock>;
  "invocation"?: InvocationId | null;
  "role": MessageRole;
  "type": "agent.message";
}
