// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * A learner's answer to an open question.
 */
export interface AnswerInput {
  "choices"?: Array<string> | null;
  "confidence"?: number | null;
  "dont_know": boolean;
  "text"?: string | null;
  "thinking"?: string | null;
}
