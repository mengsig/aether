// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * What a notification asks of the operator. Questions and blockers wait on
 * them; the rest inform.
 */
export type NotificationKind = "question" | "blocker" | "decision" | "outcome" | "info";
