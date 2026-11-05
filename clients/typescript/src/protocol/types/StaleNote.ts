// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * Why retrieval demotes a memory as possibly stale, and since when.
 */
export interface StaleNote {
  "reason": string;
  "revision"?: string | null;
  "since": string;
}
