// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  RemoteMount,
} from "../../protocol.ts";

/**
 * The runtime-changeable remote settings, saved whole under a revision.
 */
export interface RemoteSettings {
  "enabled"?: boolean;
  "mounts"?: Array<RemoteMount> | null;
  "pairing"?: boolean;
  "public_origins"?: Array<string>;
  "push"?: boolean;
}
