// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  InvocationId,
  MemoryId,
  MemoryScope,
  TaskId,
} from "../../protocol.ts";

/**
 * Every memory of a scope, or of the store, as a JSON Lines artifact.
 * Needs `memory_v2`.
 */
export interface MemoryExportCommand {
  "cmd": "memory_export";
  "scope"?: MemoryScope | null;
}

export function memoryExport(args: Omit<MemoryExportCommand, "cmd">): MemoryExportCommand {
  return { cmd: "memory_export", ...args };
}

/**
 * Newest evidence first, including retractions. Needs `memory_feedback`.
 */
export interface MemoryFeedbackCommand {
  "cmd": "memory_feedback";
  "limit"?: number | null;
  "memory": MemoryId;
}

export function memoryFeedback(args: Omit<MemoryFeedbackCommand, "cmd">): MemoryFeedbackCommand {
  return { cmd: "memory_feedback", ...args };
}

/**
 * Insert what an export holds and the store lacks; never overwrites.
 * Needs `memory_v2`.
 */
export interface MemoryImportCommand {
  "cmd": "memory_import";
  "artifact": string;
  "dry_run": boolean;
}

export function memoryImport(args: Omit<MemoryImportCommand, "cmd">): MemoryImportCommand {
  return { cmd: "memory_import", ...args };
}

/**
 * The store's Memory 2.0 migrations, counts and backups. Needs `memory_v2`.
 */
export interface MemoryMigrationStatusCommand {
  "cmd": "memory_migration_status";
}

export function memoryMigrationStatus(): MemoryMigrationStatusCommand {
  return { cmd: "memory_migration_status" };
}

/**
 * What retrieval did for each model call of a task, or for one
 * invocation; name exactly one. Needs `memory_v2`.
 */
export interface MemoryRetrievalExplainCommand {
  "cmd": "memory_retrieval_explain";
  "invocation"?: InvocationId | null;
  "task"?: TaskId | null;
}

export function memoryRetrievalExplain(args: Omit<MemoryRetrievalExplainCommand, "cmd">): MemoryRetrievalExplainCommand {
  return { cmd: "memory_retrieval_explain", ...args };
}
