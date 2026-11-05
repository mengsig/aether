// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ImportConflict,
} from "../../protocol.ts";

export interface MemoryImport {
  "conflicts"?: Array<ImportConflict>;
  "digest": string;
  "dry_run": boolean;
  "inserted": number;
  "records": number;
  "unchanged": number;
}
