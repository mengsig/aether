// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  MountKind,
} from "../../protocol.ts";

/**
 * One prefix of the listener forwarded to a loopback target.
 */
export interface RemoteMount {
  "kind": MountKind;
  "prefix": string;
  "target": string;
}
