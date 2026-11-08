// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AnswerKey,
} from "../../protocol.ts";

export interface QuestionResult {
  "answer_key"?: AnswerKey | null;
  "correct"?: boolean | null;
  "explanation"?: string | null;
  "misconception"?: string | null;
  "question": string;
}
