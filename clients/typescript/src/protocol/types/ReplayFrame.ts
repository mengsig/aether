// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  ReplayApproval,
  ReplayContext,
  ReplayMessage,
  ReplayNote,
  ReplayTool,
  TurnOutcome,
  Usage,
} from "../../protocol.ts";

/**
 * One turn of one agent.
 */
export interface ReplayFrame {
  "agent": AgentId;
  "approvals": Array<ReplayApproval>;
  "context"?: ReplayContext | null;
  "duration_ms": number;
  "ended_at"?: string | null;
  "from_seq": number;
  "messages": Array<ReplayMessage>;
  "notes"?: Array<ReplayNote>;
  "outcome": TurnOutcome;
  "started_at": string;
  "to_seq": number;
  "tools": Array<ReplayTool>;
  "turn": number;
  "usage": Usage;
}
