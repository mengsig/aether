// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * A capability band within one provider, weakest first. The vocabulary is
 * shared with Archon's catalog tiers: keep the names stable. `strongest` is
 * the legacy name of `strong` and is still accepted when reading.
 */
export type ModelTier = "fast" | "balanced" | "strong" | "frontier";
