// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * What a question does in the lesson: a probe measures, a teach-phase
 * question predicts before a step or checks after it.
 */
export type QuestionStep = "measure" | "predict" | "check" | "goal" | "practice";
