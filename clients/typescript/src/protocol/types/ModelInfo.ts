// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  CapabilityProfile,
  Effort,
  ModelRef,
} from "../../protocol.ts";

export interface ModelInfo {
  "aliases"?: Array<string>;
  "auth_source"?: string | null;
  "capabilities": CapabilityProfile;
  "configured": boolean;
  "efforts"?: Array<Effort> | null;
  "is_default": boolean;
  "model": ModelRef;
  "name": string;
  "provider_name": string;
  "superseded_by"?: string | null;
  "tier"?: string | null;
}
