// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  BillingSettings,
  BudgetSettings,
  ConcurrencySettings,
  DisableSettings,
  ExtractionSettings,
  ModelSettings,
  PrioritySettings,
  PromptSettings,
} from "../../protocol.ts";

/**
 * The `[noesis]` config table (§6.1) as one JSON object of the same shape.
 */
export interface NoesisSettingsView {
  "billing": BillingSettings;
  "budget": BudgetSettings;
  "concurrency": ConcurrencySettings;
  "disable": DisableSettings;
  "enabled": boolean;
  "engineering_context": boolean;
  "extraction": ExtractionSettings;
  "models": ModelSettings;
  "priority": PrioritySettings;
  "prompts": PromptSettings;
}
