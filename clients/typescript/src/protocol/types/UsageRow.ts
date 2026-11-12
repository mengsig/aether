// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Usage,
} from "../../protocol.ts";

/**
 * One group's spend.
 */
export interface UsageRow {
  "invocations": number;
  "key": string;
  "usage": Usage;
}
