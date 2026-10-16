// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  McpAuthState,
} from "../../protocol.ts";

/**
 * How a server authenticates. Names the variable, never its value.
 */
export interface McpAuth {
  "state": McpAuthState;
  "token_env": string;
}
