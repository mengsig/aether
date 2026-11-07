// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  BillingPatch,
  BudgetPatch,
  ConcurrencyPatch,
  DisablePatch,
  ExtractionPatch,
  ModelsPatch,
  PriorityPatch,
  PromptsPatch,
} from "../../protocol.ts";

/**
 * A subset of the `[noesis]` table to write. Absent keys keep their value.
 */
export interface NoesisSettingsPatch {
  "billing"?: BillingPatch | null;
  "budget"?: BudgetPatch | null;
  "concurrency"?: ConcurrencyPatch | null;
  "disable"?: DisablePatch | null;
  "enabled"?: boolean | null;
  "engineering_context"?: boolean | null;
  "extraction"?: ExtractionPatch | null;
  "models"?: ModelsPatch | null;
  "priority"?: PriorityPatch | null;
  "prompts"?: PromptsPatch | null;
}
