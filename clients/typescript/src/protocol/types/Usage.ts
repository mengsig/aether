// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  CostBasis,
  CostGap,
} from "../../protocol.ts";

/**
 * Token accounting for one invocation, or a running total.
 *
 * `cost_usd` sums invocations with a configured price or provider-reported
 * cost; `cost_basis` distinguishes those sources and `cost_complete` says
 * whether any invocations were left out. List prices are estimates, even
 * when every invocation was priced. A budget enforces on this accounting
 * bound - an unpriced invocation
 * cannot make what was already spent smaller - so nothing silently stops
 * firing when a model or engine reports no price.
 */
export interface Usage {
  "cache_read_tokens": number;
  "cache_write_tokens": number;
  "cost_basis"?: CostBasis;
  "cost_complete": boolean;
  "cost_gap"?: CostGap | null;
  "cost_usd": number;
  "estimated_cost_usd"?: number | null;
  "input_tokens": number;
  "output_tokens": number;
  "reasoning_tokens"?: number | null;
}
