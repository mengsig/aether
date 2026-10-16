// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  StopReason,
} from "../../protocol.ts";

/**
 * How the turn ended.
 * Tagged by `outcome`.
 */
/**
 * The log ends inside this turn: it is the turn in flight.
 */
export interface TurnOutcomeRunning {
  "outcome": "running";
}

export interface TurnOutcomeCompleted {
  "outcome": "completed";
  "stop_reason"?: StopReason | null;
}

export interface TurnOutcomeFailed {
  "error": string;
  "outcome": "failed";
  "retryable": boolean;
}

export interface TurnOutcomeInterrupted {
  "outcome": "interrupted";
}

/**
 * No turn has started: a turn-0 frame holding the notes written so far.
 */
export interface TurnOutcomeNotStarted {
  "outcome": "not_started";
}

export type TurnOutcome =
    TurnOutcomeRunning |
    TurnOutcomeCompleted |
    TurnOutcomeFailed |
    TurnOutcomeInterrupted |
    TurnOutcomeNotStarted
;
