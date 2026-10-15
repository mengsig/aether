// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ModelCostTier,
} from "../../protocol.ts";

/**
 * Price per million tokens in USD.
 */
export interface ModelCost {
  "above_threshold"?: ModelCostTier | null;
  "cache_read_per_mtok"?: number;
  "cache_write_1h_per_mtok"?: number | null;
  "cache_write_per_mtok"?: number;
  "input_per_mtok": number;
  "output_per_mtok": number;
}
