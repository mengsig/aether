// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Job,
  JobStats,
  MissClass,
  NoesisSession,
  NoesisSettingsView,
  Phase,
  PlanTree,
  PracticeOutcome,
  Problem,
  Question,
  QuestionResult,
  SessionStall,
} from "../../protocol.ts";


export interface EventKindNoesisSessionCreated {
  "session": NoesisSession;
  "type": "noesis.session_created";
}

export interface EventKindNoesisPhaseChanged {
  "from": Phase;
  "session": string;
  "to": Phase;
  "type": "noesis.phase_changed";
}

export interface EventKindNoesisQuestionPosed {
  "question": Question;
  "session": string;
  "type": "noesis.question_posed";
}

export interface EventKindNoesisAnswerRecorded {
  "result": QuestionResult;
  "session": string;
  "type": "noesis.answer_recorded";
}

export interface EventKindNoesisPlanUpdated {
  "plan": PlanTree;
  "session": string;
  "type": "noesis.plan_updated";
}

export interface EventKindNoesisProblemPosed {
  "problem": Problem;
  "session": string;
  "type": "noesis.problem_posed";
}

/**
 * The only place a problem's sealed solution is revealed.
 */
export interface EventKindNoesisPracticeConcluded {
  "miss_class"?: MissClass | null;
  "outcome": PracticeOutcome;
  "problem": string;
  "retest": boolean;
  "rung"?: number | null;
  "session": string;
  "solution": string;
  "type": "noesis.practice_concluded";
}

/**
 * A turn stopped before any model call; the session carries it too.
 */
export interface EventKindNoesisSessionStalled {
  "session": string;
  "stall": SessionStall;
  "type": "noesis.session_stalled";
}

export interface EventKindNoesisJobUpdated {
  "job": Job;
  "type": "noesis.job_updated";
}

export interface EventKindNoesisGraphMerged {
  "job": string;
  "stats": JobStats;
  "type": "noesis.graph_merged";
}

export interface EventKindNoesisLearnerUpdated {
  "concepts": Array<string>;
  "type": "noesis.learner_updated";
}

export interface EventKindNoesisSettingsChanged {
  "effective": NoesisSettingsView;
  "settings": NoesisSettingsView;
  "type": "noesis.settings_changed";
}
