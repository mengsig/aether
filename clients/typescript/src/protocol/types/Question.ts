// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  QuestionKind,
  QuestionOption,
  QuestionResult,
} from "../../protocol.ts";

/**
 * A question put to the learner. The answer key is never part of it: it
 * is revealed in a [`QuestionResult`] once the answer is recorded.
 */
export interface Question {
  "collapsed": boolean;
  "concepts": Array<string>;
  "id": string;
  "kind": QuestionKind;
  "multi": boolean;
  "options"?: Array<QuestionOption> | null;
  "plan_node"?: string | null;
  "posed_at": string;
  "prompt": string;
}
