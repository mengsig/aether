// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * Where a memory stands. The store keeps only `Candidate` or `Active`:
 * superseded and retracted are read from the record, which already says so.
 */
export type MemoryState = "candidate" | "active" | "superseded" | "retracted";
