// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * Host-only filesystem maintenance. Preview never deletes files.
 */
export interface StorageGcCommand {
  "cmd": "storage_gc";
  "dry_run": boolean;
  "prune_build_output"?: boolean;
}

export function storageGc(args: Omit<StorageGcCommand, "cmd">): StorageGcCommand {
  return { cmd: "storage_gc", ...args };
}

/**
 * Bounded filesystem inventory and eligibility preview.
 */
export interface StorageStatusCommand {
  "cmd": "storage_status";
}

export function storageStatus(): StorageStatusCommand {
  return { cmd: "storage_status" };
}
