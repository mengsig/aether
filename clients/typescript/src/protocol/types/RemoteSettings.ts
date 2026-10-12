// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * The runtime-changeable remote settings, saved whole under a revision.
 */
export interface RemoteSettings {
  "enabled"?: boolean;
  "pairing"?: boolean;
  "public_origins"?: Array<string>;
  "push"?: boolean;
}
