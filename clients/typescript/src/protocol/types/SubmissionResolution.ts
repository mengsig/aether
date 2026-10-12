// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  Reply,
  UnprovenAbsence,
} from "../../protocol.ts";

/**
 * What became of a keyed command, as far as the daemon can prove. Each
 * carries the daemon's `horizon` now; `retained_since` is for people to read.
 * Tagged by `outcome`.
 */
/**
 * It ran. `reply` is what a duplicate under the same key is answered.
 */
export interface SubmissionResolutionExecuted {
  "horizon": string;
  "outcome": "executed";
  "reply": Reply;
}

/**
 * It never ran: no receipt, and no receipt recorded since the request's
 * horizon was ever dropped. Sending it again under the same key is safe.
 */
export interface SubmissionResolutionAbsentSafe {
  "horizon": string;
  "outcome": "absent_safe";
  "retained_since": string;
}

/**
 * No receipt, and nothing proves there never was one. Only a person can
 * decide whether to send it again.
 */
export interface SubmissionResolutionUnknown {
  "horizon": string;
  "outcome": "unknown";
  "reason": UnprovenAbsence;
  "retained_since": string;
}

export type SubmissionResolution =
    SubmissionResolutionExecuted |
    SubmissionResolutionAbsentSafe |
    SubmissionResolutionUnknown
;
