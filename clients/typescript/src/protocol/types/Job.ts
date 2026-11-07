// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  JobKind,
  JobState,
  JobStats,
  Usage,
} from "../../protocol.ts";

export interface Job {
  "created_at": string;
  "error"?: string | null;
  "id": string;
  "kind": JobKind;
  "state": JobState;
  "stats"?: JobStats | null;
  "target": string;
  "updated_at": string;
  "usage": Usage;
}
