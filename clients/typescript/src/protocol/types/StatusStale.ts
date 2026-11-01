// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  DaemonStatus,
} from "../../protocol.ts";

/**
 * Why a [`DaemonStatus`] is not fresh, and how old it is.
 */
export interface StatusStale {
  "read_at"?: string | null;
  "waited_ms": number;
}
