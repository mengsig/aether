// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  WindowHeadroom,
} from "../../protocol.ts";

/**
 * Read-time projection of the latest report; unknown usage is always stale.
 */
export interface ProviderHeadroom {
  "age_seconds"?: number | null;
  "observed_at"?: string | null;
  "stale": boolean;
  "windows": Array<WindowHeadroom>;
}
