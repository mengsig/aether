// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AttachmentRef,
  ReferenceKind,
} from "../../protocol.ts";

/**
 * One checked reference, as the task stores it.
 */
export interface TaskReference {
  "image"?: AttachmentRef | null;
  "kind": ReferenceKind;
  "label": string;
  "path": string;
  "sha256": string;
  "size_bytes": number;
}
