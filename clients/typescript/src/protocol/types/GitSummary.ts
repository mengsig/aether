// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


export interface GitSummary {
  "ahead"?: number | null;
  "behind"?: number | null;
  "branch"?: string | null;
  "detached"?: boolean;
  "dirty"?: boolean;
  "head"?: string | null;
  "modified"?: number;
  "repository": boolean;
  "root"?: string | null;
  "staged"?: number;
  "untracked"?: number;
}
