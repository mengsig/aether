// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * A memory's current tally and the factor it gives, as the host reports it.
 */
export interface FeedbackRanking {
  "factor": number;
  "helped": number;
  "hurt": number;
}
