// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  PathCompletionKind,
} from "../../protocol.ts";

/**
 * Complete a partial path on the daemon's machine. Bounded: at most
 * `limit` entries (default 50, never more than 200).
 */
export interface CompletePathCommand {
  "cmd": "complete_path";
  "kind": PathCompletionKind;
  "limit"?: number | null;
  "prefix": string;
  "workspace"?: string | null;
}

export function completePath(args: Omit<CompletePathCommand, "cmd">): CompletePathCommand {
  return { cmd: "complete_path", ...args };
}
