// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
} from "../../protocol.ts";

/**
 * The result of an operator's integration. Refused with `conflicts` when
 * the source changed a path the child also changed; nothing was written.
 */
export interface WorkspaceIntegration {
  "agent": AgentId;
  "already_integrated": boolean;
  "changed_paths": Array<string>;
  "conflicts": Array<string>;
  "integrated": boolean;
  "source": string;
}
