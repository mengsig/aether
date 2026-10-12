// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AttachmentId,
} from "../../protocol.ts";

/**
 * A stored image, as inboxes, events and transcripts carry it.
 */
export interface AttachmentRef {
  "id": AttachmentId;
  "media_type": string;
  "name"?: string | null;
  "size_bytes": number;
}
