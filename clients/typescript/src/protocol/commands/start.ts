// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  DeviceRole,
} from "../../protocol.ts";

/**
 * Open a pairing and mint its single-use code. Host only.
 */
export interface StartPairingCommand {
  "cmd": "start_pairing";
  "role"?: DeviceRole;
}

export function startPairing(args: Omit<StartPairingCommand, "cmd">): StartPairingCommand {
  return { cmd: "start_pairing", ...args };
}
