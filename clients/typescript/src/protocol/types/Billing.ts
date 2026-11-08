// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * What paid for a usage record when it was written: an engine CLI on the
 * user's own subscription, or a provider billed per request.
 */
export type Billing = "subscription" | "api";
