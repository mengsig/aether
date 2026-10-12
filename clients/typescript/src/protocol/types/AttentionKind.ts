// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * What can wait on the operator, and what a push notification is about.
 */
export type AttentionKind = "approval" | "question" | "agent_failed" | "agent_blocked" | "task_completed" | "task_failed" | "test";
