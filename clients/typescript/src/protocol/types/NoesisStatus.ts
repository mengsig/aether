// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  DisablePolicy,
  NoesisModelChoice,
  NoesisModelOffer,
  NoesisSettingsView,
  Reply,
  SpendToday,
} from "../../protocol.ts";

/**
 * Reply to `noesis_status` and `noesis_configure`.
 */
export interface NoesisStatus {
  "disable_policy": DisablePolicy;
  "effective": NoesisSettingsView;
  "enabled": boolean;
  "jobs_running": number;
  "models"?: Array<NoesisModelOffer>;
  "prompt_overrides": Array<string>;
  "settings": NoesisSettingsView;
  "settings_pending"?: boolean;
  "spend_today": SpendToday;
  "teaching_model"?: NoesisModelChoice | null;
}
