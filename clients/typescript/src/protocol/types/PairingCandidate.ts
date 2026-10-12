// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * The device asking to be enrolled, as the host must confirm it.
 */
export interface PairingCandidate {
  "claimed_at": string;
  "fingerprint": string;
  "name": string;
  "phrase": string;
}
