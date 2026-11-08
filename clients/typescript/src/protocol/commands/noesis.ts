// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  AnswerInput,
  ArtifactId,
  ConceptStateKind,
  DeleteSessionGraph,
  DeleteSourceGraph,
  EvidenceBundle,
  JobState,
  NoesisSettingsPatch,
  SessionMode,
  status,
} from "../../protocol.ts";

/**
 * Answer the session's open question.
 */
export interface NoesisAnswerCommand {
  "cmd": "noesis_answer";
  "answer": AnswerInput;
  "idempotency_key"?: string | null;
  "question": string;
  "session": string;
}

export function noesisAnswer(args: Omit<NoesisAnswerCommand, "cmd">): NoesisAnswerCommand {
  return { cmd: "noesis_answer", ...args };
}

export interface NoesisCancelJobCommand {
  "cmd": "noesis_cancel_job";
  "idempotency_key"?: string | null;
  "job": string;
}

export function noesisCancelJob(args: Omit<NoesisCancelJobCommand, "cmd">): NoesisCancelJobCommand {
  return { cmd: "noesis_cancel_job", ...args };
}

/**
 * Write a subset of the `[noesis]` settings and answer with the new
 * status. Works while Noesis is disabled. Host only when the patch sets
 * `enabled`, a budget, billing or prompt overrides.
 */
export interface NoesisConfigureCommand {
  "cmd": "noesis_configure";
  "idempotency_key"?: string | null;
  "patch": NoesisSettingsPatch;
}

export function noesisConfigure(args: Omit<NoesisConfigureCommand, "cmd">): NoesisConfigureCommand {
  return { cmd: "noesis_configure", ...args };
}

/**
 * Record the operator's correction of a concept's state.
 */
export interface NoesisCorrectLearnerCommand {
  "cmd": "noesis_correct_learner";
  "concept": string;
  "idempotency_key"?: string | null;
  "reason": string;
  "state": ConceptStateKind;
}

export function noesisCorrectLearner(args: Omit<NoesisCorrectLearnerCommand, "cmd">): NoesisCorrectLearnerCommand {
  return { cmd: "noesis_correct_learner", ...args };
}

/**
 * Start a teaching or practice session. Answered with `noesis_session`.
 */
export interface NoesisCreateSessionCommand {
  "cmd": "noesis_create_session";
  "context"?: EvidenceBundle | null;
  "goal": string;
  "idempotency_key"?: string | null;
  "mode": SessionMode;
  "practice_targets"?: Array<string> | null;
}

export function noesisCreateSession(args: Omit<NoesisCreateSessionCommand, "cmd">): NoesisCreateSessionCommand {
  return { cmd: "noesis_create_session", ...args };
}

/**
 * Delete a proposition; its chains go with it.
 */
export interface NoesisDeletePropositionCommand {
  "cmd": "noesis_delete_proposition";
  "idempotency_key"?: string | null;
  "proposition": string;
}

export function noesisDeleteProposition(args: Omit<NoesisDeletePropositionCommand, "cmd">): NoesisDeletePropositionCommand {
  return { cmd: "noesis_delete_proposition", ...args };
}

export interface NoesisDeleteSessionCommand {
  "cmd": "noesis_delete_session";
  "graph": DeleteSessionGraph;
  "idempotency_key"?: string | null;
  "session": string;
}

export function noesisDeleteSession(args: Omit<NoesisDeleteSessionCommand, "cmd">): NoesisDeleteSessionCommand {
  return { cmd: "noesis_delete_session", ...args };
}

/**
 * Refused as a conflict while a job on the source runs.
 */
export interface NoesisDeleteSourceCommand {
  "cmd": "noesis_delete_source";
  "graph": DeleteSourceGraph;
  "idempotency_key"?: string | null;
  "source": string;
}

export function noesisDeleteSource(args: Omit<NoesisDeleteSourceCommand, "cmd">): NoesisDeleteSourceCommand {
  return { cmd: "noesis_delete_source", ...args };
}

/**
 * Extract the graph of exactly one of `session` and `source`.
 */
export interface NoesisExtractCommand {
  "cmd": "noesis_extract";
  "idempotency_key"?: string | null;
  "session"?: string | null;
  "source"?: string | null;
}

export function noesisExtract(args: Omit<NoesisExtractCommand, "cmd">): NoesisExtractCommand {
  return { cmd: "noesis_extract", ...args };
}

export interface NoesisGetSessionCommand {
  "cmd": "noesis_get_session";
  "session": string;
}

export function noesisGetSession(args: Omit<NoesisGetSessionCommand, "cmd">): NoesisGetSessionCommand {
  return { cmd: "noesis_get_session", ...args };
}

/**
 * Propositions, chains and clusters, filtered by text, session or
 * source. `limit` is 1 to 5000.
 */
export interface NoesisGraphCommand {
  "cmd": "noesis_graph";
  "limit"?: number | null;
  "query"?: string | null;
  "session"?: string | null;
  "source"?: string | null;
}

export function noesisGraph(args: Omit<NoesisGraphCommand, "cmd">): NoesisGraphCommand {
  return { cmd: "noesis_graph", ...args };
}

/**
 * Import an artifact as a source and extract it.
 */
export interface NoesisImportSourceCommand {
  "cmd": "noesis_import_source";
  "artifact": ArtifactId;
  "idempotency_key"?: string | null;
  "name": string;
}

export function noesisImportSource(args: Omit<NoesisImportSourceCommand, "cmd">): NoesisImportSourceCommand {
  return { cmd: "noesis_import_source", ...args };
}

export interface NoesisJobsCommand {
  "cmd": "noesis_jobs";
  "state"?: JobState | null;
}

export function noesisJobs(args: Omit<NoesisJobsCommand, "cmd">): NoesisJobsCommand {
  return { cmd: "noesis_jobs", ...args };
}

/**
 * Learner state of the named concepts, or of those matching `query`.
 * `limit` is 1 to 1000.
 */
export interface NoesisLearnerCommand {
  "cmd": "noesis_learner";
  "concepts"?: Array<string> | null;
  "limit"?: number | null;
  "query"?: string | null;
}

export function noesisLearner(args: Omit<NoesisLearnerCommand, "cmd">): NoesisLearnerCommand {
  return { cmd: "noesis_learner", ...args };
}

/**
 * A concept's evidence, newest first, from before the evidence id
 * `before`. `limit` is 1 to 500.
 */
export interface NoesisLearnerEvidenceCommand {
  "cmd": "noesis_learner_evidence";
  "before"?: number | null;
  "concept": string;
  "limit"?: number | null;
}

export function noesisLearnerEvidence(args: Omit<NoesisLearnerEvidenceCommand, "cmd">): NoesisLearnerEvidenceCommand {
  return { cmd: "noesis_learner_evidence", ...args };
}

/**
 * One page of sessions, from just after `after`. `limit` is 1 to 200.
 */
export interface NoesisListSessionsCommand {
  "cmd": "noesis_list_sessions";
  "after"?: string | null;
  "limit"?: number | null;
}

export function noesisListSessions(args: Omit<NoesisListSessionsCommand, "cmd">): NoesisListSessionsCommand {
  return { cmd: "noesis_list_sessions", ...args };
}

/**
 * Retry a stalled teaching turn after changing its budget.
 */
export interface NoesisRetryTurnCommand {
  "cmd": "noesis_retry_turn";
  "idempotency_key"?: string | null;
  "session": string;
}

export function noesisRetryTurn(args: Omit<NoesisRetryTurnCommand, "cmd">): NoesisRetryTurnCommand {
  return { cmd: "noesis_retry_turn", ...args };
}

/**
 * Say something to the session's teacher. 1 to 20000 characters.
 */
export interface NoesisSendCommand {
  "cmd": "noesis_send";
  "idempotency_key"?: string | null;
  "session": string;
  "text": string;
}

export function noesisSend(args: Omit<NoesisSendCommand, "cmd">): NoesisSendCommand {
  return { cmd: "noesis_send", ...args };
}

/**
 * Noesis state: whether it is enabled, its settings as stored and as
 * effective, running jobs and today's spend. Needs `features::NOESIS`.
 */
export interface NoesisStatusCommand {
  "cmd": "noesis_status";
}

export function noesisStatus(): NoesisStatusCommand {
  return { cmd: "noesis_status" };
}
