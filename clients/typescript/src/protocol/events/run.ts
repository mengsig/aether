// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  RunId,
  Usage,
} from "../../protocol.ts";

/**
 * A run was replayed into this store from another daemon's export. It is
 * the imported run's first event, so everything after it reads as
 * history that happened elsewhere.
 */
export interface EventKindRunImported {
  "events": number;
  "exported_at": string;
  "source_instance": string;
  "source_run": RunId;
  "type": "run.imported";
}
/**
 * A run crossed its hard spend stop: every live agent in it was
 * interrupted (not failed) and no new turn is admitted until the limit
 * is raised.
 */
export interface EventKindRunHardStop {
  "interrupted": number;
  "limit": string;
  "reason": string;
  "type": "run.hard_stop";
  "usage": Usage;
}
