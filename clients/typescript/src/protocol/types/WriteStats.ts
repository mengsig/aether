// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  KindWrites,
} from "../../protocol.ts";

/**
 * Payload bytes the store committed since `since`, heaviest kind first.
 * Counts JSON bodies, not SQLite pages: indexes and the WAL add on top, so
 * the file sizes are reported next to it.
 */
export interface WriteStats {
  "db_bytes"?: number | null;
  "kinds": Array<KindWrites>;
  "since"?: string | null;
  "total_bytes": number;
  "total_rows": number;
  "wal_bytes"?: number | null;
}
