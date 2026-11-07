// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * The learner's answer as recorded.
 */
export interface RecordedAnswer {
  "answered_at": string;
  "chosen"?: Array<string>;
  "confidence"?: number | null;
  "dont_know": boolean;
  "text"?: string | null;
  "thinking"?: string | null;
}
