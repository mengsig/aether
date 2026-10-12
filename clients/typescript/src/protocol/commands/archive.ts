// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ArchiveTarget,
  replay,
} from "../../protocol.ts";

/**
 * Clear finished work out of the live view, or bring it back. The mark
 * is durable and hides nothing from the store, reports or replay.
 * Only a finished agent can be archived.
 */
export interface ArchiveCommand {
  "cmd": "archive";
  "archived"?: boolean;
  "idempotency_key"?: string | null;
  "target": ArchiveTarget;
}

export function archive(args: Omit<ArchiveCommand, "cmd">): ArchiveCommand {
  return { cmd: "archive", ...args };
}
