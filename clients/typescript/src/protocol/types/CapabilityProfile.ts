// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ModelCost,
  ThinkingSupport,
} from "../../protocol.ts";

/**
 * What a model can do. Declared per model in the registry so callers can ask
 * before they depend on a feature.
 */
export interface CapabilityProfile {
  "compaction_tokens"?: number | null;
  "context_window": number;
  "cost"?: ModelCost | null;
  "effort"?: boolean;
  "extensions"?: Record<string, unknown>;
  "max_output_tokens": number;
  "parallel_tools"?: boolean;
  "prompt_caching"?: boolean;
  "strict_tools"?: boolean;
  "structured_output"?: boolean;
  "thinking"?: ThinkingSupport;
  "tools"?: boolean;
  "vision"?: boolean;
}
