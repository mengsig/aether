// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  DeviceId,
  DeviceRole,
  Welcome,
} from "../../protocol.ts";

/**
 * The device a connection authenticated as, echoed in [`crate::Welcome`].
 */
export interface DeviceIdentity {
  "id": DeviceId;
  "name": string;
  "role": DeviceRole;
}
