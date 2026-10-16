// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ProviderHeadroom,
  ProviderHealth,
} from "../../protocol.ts";

/**
 * One provider as a client sees it: what it is, whether it can be used, and
 * how it is behaving right now. `health` is live scheduler state, so a
 * provider that has done nothing since the daemon started reads as idle.
 */
export interface ProviderInfo {
  "auth_source"?: string | null;
  "base_url"?: string;
  "configured": boolean;
  "enabled": boolean;
  "engine": boolean;
  "family"?: string | null;
  "headroom"?: ProviderHeadroom;
  "health": ProviderHealth;
  "models": number;
  "name": string;
  "open_model_list": boolean;
  "protocol": string;
  "provider": string;
}
