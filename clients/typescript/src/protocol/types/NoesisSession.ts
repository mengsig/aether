// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  AskedQuestion,
  EvidenceBundle,
  Phase,
  PlanTree,
  PosedProblem,
  Problem,
  Question,
  SessionAgent,
  SessionMode,
  SessionStall,
  SessionUsage,
} from "../../protocol.ts";

export interface NoesisSession {
  "active_problem"?: Problem | null;
  "agent": AgentId;
  "agents"?: Array<SessionAgent>;
  "context"?: EvidenceBundle | null;
  "created_at": string;
  "goal": string;
  "id": string;
  "mode": SessionMode;
  "open_question"?: Question | null;
  "phase": Phase;
  "plan"?: PlanTree | null;
  "problems"?: Array<PosedProblem>;
  "questions"?: Array<AskedQuestion>;
  "stalled"?: SessionStall | null;
  "updated_at": string;
  "usage"?: SessionUsage | null;
}
