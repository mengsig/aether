// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Effort,
  ThinkingMode,
} from "../../protocol.ts";

export interface InvocationParams {
  "effort"?: Effort | null;
  "effort_effective"?: Effort | null;
  "effort_requested"?: Effort | null;
  "extensions"?: Record<string, unknown>;
  "max_output_tokens"?: number | null;
  "temperature"?: number | null;
  "thinking"?: ThinkingMode | null;
}
