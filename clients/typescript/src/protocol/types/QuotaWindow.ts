// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * A subscription usage window as an engine reported it. Opaque JSON on the
 * wire from the CLI; normalized here so every reader agrees.
 */
export interface QuotaWindow {
  "resets_at"?: string | null;
  "used_percent": number;
  "window_minutes"?: number | null;
}
