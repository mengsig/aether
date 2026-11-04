// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  ConceptState,
  NoesisSession,
  NoesisStatus,
} from "../../protocol.ts";


export interface ReplyNoesisStatus {
  "data": NoesisStatus;
  "reply": "noesis_status";
}

export interface ReplyNoesisSession {
  "data": NoesisSession;
  "reply": "noesis_session";
}

/**
 * `next_after` is the cursor for the next page, absent on the last one.
 */
export interface ReplyNoesisSessions {
  "data": Record<string, unknown>;
  "reply": "noesis_sessions";
}

/**
 * `truncated` is set when `limit` cut the graph short.
 */
export interface ReplyNoesisGraph {
  "data": Record<string, unknown>;
  "reply": "noesis_graph";
}

export interface ReplyNoesisJob {
  "data": Record<string, unknown>;
  "reply": "noesis_job";
}

export interface ReplyNoesisImport {
  "data": Record<string, unknown>;
  "reply": "noesis_import";
}

export interface ReplyNoesisJobs {
  "data": Record<string, unknown>;
  "reply": "noesis_jobs";
}

export interface ReplyNoesisLearner {
  "data": Record<string, unknown>;
  "reply": "noesis_learner";
}

/**
 * `next_before` is the cursor for the next page, absent on the last one.
 */
export interface ReplyNoesisEvidence {
  "data": Record<string, unknown>;
  "reply": "noesis_evidence";
}

export interface ReplyConceptState {
  "data": ConceptState;
  "reply": "concept_state";
}
