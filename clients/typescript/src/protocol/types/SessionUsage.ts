// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Usage,
} from "../../protocol.ts";

/**
 * A session's usage by billing: subscription use is covered by the
 * learner's plan and never charged per request; API use is.
 */
export interface SessionUsage {
  "api": Usage;
  "subscription": Usage;
  "unknown"?: Usage;
}
