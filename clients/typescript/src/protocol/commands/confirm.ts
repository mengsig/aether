// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * Enrol or reject the device that claimed a pairing. Host only.
 */
export interface ConfirmPairingCommand {
  "cmd": "confirm_pairing";
  "accept": boolean;
  "id": string;
}

export function confirmPairing(args: Omit<ConfirmPairingCommand, "cmd">): ConfirmPairingCommand {
  return { cmd: "confirm_pairing", ...args };
}
