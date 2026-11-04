// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  PlanNodeStatus,
  status,
} from "../../protocol.ts";

export interface PlanNode {
  "id": string;
  "status": PlanNodeStatus;
  "title": string;
}
