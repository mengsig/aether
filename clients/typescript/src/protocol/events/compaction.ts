// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  CheckpointId,
  CompactionTrigger,
  ModelRef,
  compact,
  trigger,
} from "../../protocol.ts";


export interface EventKindCompactionRequested {
  "trigger": CompactionTrigger;
  "type": "compaction.requested";
}

export interface EventKindCompactionCompleted {
  "after_tokens"?: number | null;
  "before_tokens"?: number | null;
  "checkpoint": CheckpointId;
  "from_seq": number;
  "to_seq": number;
  "type": "compaction.completed";
  "version": number;
}

/**
 * The engine or provider serving the agent folded the context itself
 * (Claude Code's `/compact`, Anthropic's server-side summary). No
 * checkpoint exists: the summary lives in the engine session or the transcript.
 */
export interface EventKindCompactionNative {
  "after_tokens"?: number | null;
  "automatic"?: boolean;
  "before_tokens"?: number | null;
  "duration_ms"?: number | null;
  "provider": string;
  "type": "compaction.native";
}

/**
 * `models` names what was asked to fold, in the order it was tried: a
 * transcript that reached a provider that then failed leaves no other
 * durable trace of the egress.
 */
export interface EventKindCompactionFailed {
  "error": string;
  "models"?: Array<ModelRef>;
  "type": "compaction.failed";
}
