// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  DisablePolicy,
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
  "prompt_overrides": Array<string>;
  "settings": NoesisSettingsView;
  "spend_today": SpendToday;
}
