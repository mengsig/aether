// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  DeviceId,
  DeviceRole,
} from "../../protocol.ts";

/**
 * One enrolled device as `aether remote devices` and a phone's settings list it.
 */
export interface Device {
  "connections"?: number;
  "created_at": string;
  "current"?: boolean;
  "fingerprint"?: string | null;
  "id": DeviceId;
  "last_seen_at"?: string | null;
  "name": string;
  "push"?: boolean;
  "revoked_at"?: string | null;
  "role": DeviceRole;
}
