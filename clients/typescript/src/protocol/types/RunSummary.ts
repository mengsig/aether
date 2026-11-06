// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  RunId,
  Usage,
} from "../../protocol.ts";

/**
 * What one run has spent and where that rate leads.
 */
export interface RunSummary {
  "agents": number;
  "agents_live": number;
  "forecast_usd_next_hour": number;
  "rate_approximate"?: boolean;
  "rate_usd_per_hour": number;
  "run": RunId;
  "started_at": string;
  "usage": Usage;
}
