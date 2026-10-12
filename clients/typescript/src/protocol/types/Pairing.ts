// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  DeviceIdentity,
  DeviceRole,
  PairingCandidate,
  PairingState,
  Reply,
} from "../../protocol.ts";

/**
 * One pairing attempt. Reply to the four pairing commands.
 */
export interface Pairing {
  "candidate"?: PairingCandidate | null;
  "code"?: string | null;
  "device"?: DeviceIdentity | null;
  "expires_at": string;
  "id": string;
  "role": DeviceRole;
  "state": PairingState;
  "url"?: string | null;
}
