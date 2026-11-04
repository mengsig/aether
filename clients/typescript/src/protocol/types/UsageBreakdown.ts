// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Reply,
  Usage,
  UsageRow,
} from "../../protocol.ts";

/**
 * Reply to `usage_breakdown`: rows ordered by `key`, and the total over
 * every invocation in scope, those of rows cut by `limit` included.
 */
export interface UsageBreakdown {
  "rows": Array<UsageRow>;
  "total": Usage;
  "truncated": boolean;
}
