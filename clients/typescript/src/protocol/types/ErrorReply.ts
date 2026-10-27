// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ErrorCode,
} from "../../protocol.ts";

export interface ErrorReply {
  "code": ErrorCode;
  "first_seq"?: number | null;
  "last_seq"?: number | null;
  "message": string;
}
