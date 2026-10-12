// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  GitSummary,
} from "../../protocol.ts";

/**
 * One workspace the daemon knows: it holds or held a root agent.
 */
export interface Workspace {
  "active_at": string;
  "agents": number;
  "exists": boolean;
  "git"?: GitSummary | null;
  "path": string;
  "running": number;
}
