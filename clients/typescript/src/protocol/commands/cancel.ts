// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AgentId,
} from "../../protocol.ts";


export interface CancelCommand {
  "cmd": "cancel";
  "agent": AgentId;
  "idempotency_key"?: string | null;
  "reason"?: string | null;
}

export function cancel(args: Omit<CancelCommand, "cmd">): CancelCommand {
  return { cmd: "cancel", ...args };
}

/**
 * Close a pairing nobody confirmed. Host only.
 */
export interface CancelPairingCommand {
  "cmd": "cancel_pairing";
  "id": string;
}

export function cancelPairing(args: Omit<CancelPairingCommand, "cmd">): CancelPairingCommand {
  return { cmd: "cancel_pairing", ...args };
}
