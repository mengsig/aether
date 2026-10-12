// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * Why an HTTP request was refused. See the table in docs/remote.md.
 */
export type HttpErrorCode = "bad_request" | "forbidden_host" | "forbidden_origin" | "not_found" | "method_not_allowed" | "unsupported_media_type" | "payload_too_large" | "pairing_closed" | "invalid_code" | "expired" | "rate_limited" | "unavailable" | "internal";
