// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
} from "../../protocol.ts";

export interface WorkspaceDiscard {
  "agent": AgentId;
  "forced": boolean;
  "workspace": string;
}
