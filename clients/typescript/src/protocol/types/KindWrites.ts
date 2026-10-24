// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * Rows and payload bytes written for one kind of record: an event type such
 * as `agent.usage_updated`, or a table such as `table:agents`.
 */
export interface KindWrites {
  "bytes": number;
  "kind": string;
  "rows": number;
}
