// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  StatusStale,
} from "../../protocol.ts";

export interface AgentTree {
  "next_after"?: AgentId | null;
  "nodes": Record<string, unknown>;
  "roots": Array<AgentId>;
  "stale"?: StatusStale | null;
}
