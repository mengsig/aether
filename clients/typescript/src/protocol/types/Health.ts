// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  status,
} from "../../protocol.ts";

/**
 * Body of `GET /api/health`.
 */
export interface Health {
  "min_protocol_version": number;
  "pairing_open": boolean;
  "protocol_version": number;
  "status": string;
}
