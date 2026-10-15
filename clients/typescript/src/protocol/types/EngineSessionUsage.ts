// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Usage,
} from "../../protocol.ts";

/**
 * An engine session's own cumulative token counters, as it last reported
 * them, tagged with the session they belong to.
 *
 * Codex counts a *thread*, not a turn: the counters it reports on a resumed
 * thread include every earlier turn. Persisting the last reading is what
 * lets the next turn bill `total - this` instead of the thread's history.
 * The session id is part of the record so a cleared or replaced session
 * cannot be billed against another session's counters.
 */
export interface EngineSessionUsage {
  "models"?: Record<string, Usage> | null;
  "session": string;
  "total": Usage;
}
