// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  Phase,
  PlanTree,
  Problem,
  Question,
  SessionMode,
} from "../../protocol.ts";

export interface NoesisSession {
  "active_problem"?: Problem | null;
  "agent": AgentId;
  "created_at": string;
  "goal": string;
  "id": string;
  "mode": SessionMode;
  "open_question"?: Question | null;
  "phase": Phase;
  "plan"?: PlanTree | null;
  "updated_at": string;
}
