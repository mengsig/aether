// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
  ChangedFile,
  WorkspaceCommit,
} from "../../protocol.ts";

/**
 * An agent's work in its workspace, bounded for the wire.
 */
export interface WorkspaceDiff {
  "agent": AgentId;
  "ambiguous": boolean;
  "commits": Array<WorkspaceCommit>;
  "commits_truncated": boolean;
  "deletions": number;
  "files": Array<ChangedFile>;
  "files_truncated": boolean;
  "insertions": number;
  "integrated": boolean;
  "isolated": boolean;
  "patch": string;
  "patch_bytes": number;
  "patch_truncated": boolean;
  "source"?: string | null;
  "workspace": string;
}
