// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  ApprovalId,
  AttentionKind,
  TaskId,
} from "../../protocol.ts";

/**
 * The decrypted body of one Web Push message.
 */
export interface PushPayload {
  "agent"?: AgentId | null;
  "agent_name"?: string | null;
  "approval"?: ApprovalId | null;
  "at": string;
  "instance_id": string;
  "kind": AttentionKind;
  "preview"?: string | null;
  "question_seq"?: number | null;
  "seq": number;
  "task"?: TaskId | null;
  "title": string;
  "url": string;
  "v": number;
}
