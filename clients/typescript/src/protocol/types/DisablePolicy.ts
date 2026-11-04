// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  BackgroundDisable,
} from "../../protocol.ts";

/**
 * What a disable does to work in flight.
 */
export interface DisablePolicy {
  "background": BackgroundDisable;
  "grace_seconds": number;
}
