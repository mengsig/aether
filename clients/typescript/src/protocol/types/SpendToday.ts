// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Usage,
} from "../../protocol.ts";

/**
 * What Noesis spent today, split by who asked for the work.
 */
export interface SpendToday {
  "api"?: Usage;
  "background": Usage;
  "interactive": Usage;
  "subscription"?: Usage;
  "unknown"?: Usage;
}
