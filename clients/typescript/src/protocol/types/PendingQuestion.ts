// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
} from "../../protocol.ts";

/**
 * An agent's question nobody has answered or superseded.
 */
export interface PendingQuestion {
  "agent": AgentId;
  "agent_name"?: string | null;
  "asked_at": string;
  "question": string;
  "question_seq": number;
}
