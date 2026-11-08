// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Question,
  QuestionResult,
  RecordedAnswer,
} from "../../protocol.ts";

/**
 * A posed question with what the learner answered and what it revealed.
 */
export interface AskedQuestion {
  "answer"?: RecordedAnswer | null;
  "question": Question;
  "result"?: QuestionResult | null;
}
