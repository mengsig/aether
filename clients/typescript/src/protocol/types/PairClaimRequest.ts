// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * Body of `POST /api/pair/claim`.
 */
export interface PairClaimRequest {
  "code": string;
  "device_name": string;
  "public_key": string;
}
