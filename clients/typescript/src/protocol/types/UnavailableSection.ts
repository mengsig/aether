// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  DaemonStatus,
} from "../../protocol.ts";

/**
 * A [`DaemonStatus`] section left out of a reply, and why.
 */
export interface UnavailableSection {
  "reason": string;
  "section": string;
}
