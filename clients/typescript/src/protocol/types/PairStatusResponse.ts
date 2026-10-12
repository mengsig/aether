// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  DeviceIdentity,
  PairingState,
} from "../../protocol.ts";

export interface PairStatusResponse {
  "device"?: DeviceIdentity | null;
  "state": PairingState;
}
