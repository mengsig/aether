// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * Why an absent receipt does not prove its key was never executed.
 */
export type UnprovenAbsence = "no_horizon" | "foreign_horizon" | "horizon_ahead" | "forgotten";
