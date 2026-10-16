// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  McpAuth,
  McpServerState,
  McpToolInfo,
} from "../../protocol.ts";

/**
 * A server's health as of the last contact attempt.
 */
export interface McpServerHealth {
  "auth"?: McpAuth | null;
  "checked_at"?: string | null;
  "error"?: string | null;
  "name": string;
  "state": McpServerState;
  "tools"?: Array<McpToolInfo>;
  "transport": string;
}
