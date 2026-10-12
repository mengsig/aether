// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  GitSummary,
} from "../../protocol.ts";

export interface WorkspaceValidation {
  "git"?: GitSummary | null;
  "path"?: string | null;
  "reason"?: string | null;
  "valid": boolean;
}
