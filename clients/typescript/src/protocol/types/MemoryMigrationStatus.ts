// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  LastImport,
  MemoryCounts,
  MemoryMigration,
  MigrationBackup,
  memories,
} from "../../protocol.ts";

export interface MemoryMigrationStatus {
  "backups"?: Array<MigrationBackup>;
  "last_import"?: LastImport | null;
  "memories": MemoryCounts;
  "migrations": Array<MemoryMigration>;
  "schema_version": number;
}
