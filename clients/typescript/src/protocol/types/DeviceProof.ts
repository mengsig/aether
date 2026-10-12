// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  DeviceId,
  Hello,
} from "../../protocol.ts";

/**
 * A device's answer to the carrier's challenge, sent in [`crate::Hello`].
 */
export interface DeviceProof {
  "id": DeviceId;
  "signature": string;
}
