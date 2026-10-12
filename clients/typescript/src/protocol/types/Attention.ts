// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentAttention,
  ApprovalRequest,
  PendingQuestion,
} from "../../protocol.ts";

/**
 * Everything waiting on the operator, read at one sequence number.
 */
export interface Attention {
  "agents": Array<AgentAttention>;
  "approvals": Array<ApprovalRequest>;
  "last_seq": number;
  "questions": Array<PendingQuestion>;
}
