// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AttentionKind,
  Reply,
} from "../../protocol.ts";

/**
 * Reply to the push commands: what the daemon sends this device.
 */
export interface PushConfig {
  "enabled": boolean;
  "kinds"?: Array<AttentionKind>;
  "previews"?: boolean;
  "subscribed"?: boolean;
  "vapid_public_key"?: string | null;
}
