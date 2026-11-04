// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  UsageGroup,
  UsageScope,
} from "../../protocol.ts";

/**
 * Spend summed from `invocation.completed` events, grouped by `by`, in
 * `since..until` (completion time, `until` exclusive). `limit` is 1 to
 * 1000 rows, 1000 when absent. Needs `features::USAGE_BREAKDOWN`.
 */
export interface UsageBreakdownCommand {
  "cmd": "usage_breakdown";
  "by": UsageGroup;
  "limit"?: number | null;
  "scope"?: UsageScope;
  "since"?: string | null;
  "until"?: string | null;
}

export function usageBreakdown(args: Omit<UsageBreakdownCommand, "cmd">): UsageBreakdownCommand {
  return { cmd: "usage_breakdown", ...args };
}
