// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  RemoteSettings,
  Reply,
} from "../../protocol.ts";

/**
 * Reply to `get_remote` and `configure_remote`.
 */
export interface RemoteStatus {
  "address": string;
  "allow_remote"?: boolean;
  "connections"?: number;
  "devices"?: number;
  "error"?: string | null;
  "listening"?: string | null;
  "push_previews"?: boolean;
  "revision": number;
  "settings": RemoteSettings;
}
