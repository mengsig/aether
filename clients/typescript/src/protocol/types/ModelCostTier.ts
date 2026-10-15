// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * Rates for an entire request whose prompt exceeds `prompt_tokens`.
 */
export interface ModelCostTier {
  "cache_read_per_mtok"?: number;
  "cache_write_1h_per_mtok"?: number | null;
  "cache_write_per_mtok"?: number;
  "input_per_mtok": number;
  "output_per_mtok": number;
  "prompt_tokens": number;
}
