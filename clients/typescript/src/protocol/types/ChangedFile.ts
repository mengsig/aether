// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  FileStatus,
  status,
} from "../../protocol.ts";

export interface ChangedFile {
  "deletions"?: number | null;
  "insertions"?: number | null;
  "path": string;
  "status": FileStatus;
}
