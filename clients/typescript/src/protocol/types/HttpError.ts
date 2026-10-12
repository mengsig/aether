// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  HttpErrorBody,
} from "../../protocol.ts";

/**
 * Every non-2xx HTTP response body: `{"error": {"code", "message"}}`.
 */
export interface HttpError {
  "error": HttpErrorBody;
}
