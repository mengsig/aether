// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  HttpErrorCode,
} from "../../protocol.ts";

export interface HttpErrorBody {
  "code": HttpErrorCode;
  "message": string;
}
