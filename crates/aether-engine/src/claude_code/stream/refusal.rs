//! Prove that a failed invocation billed nothing: the API refused it before
//! inference, and nothing earlier in the process can have been billed.
use serde_json::{Map, Value};

use super::ParseState;
use crate::claude_code::session_cost::SpendEvidence;

/// HTTP statuses the API answers before inference, which it does not bill.
/// Claude Code resends a 400 reshaped without an `api_retry` record; it
/// reports every other retry with the status that caused it.
const UNBILLED_REJECTIONS: [u64; 5] = [400, 401, 403, 404, 413];

impl ParseState {
    /// An `api_retry` record: the attempt before it is unbilled only when the
    /// API refused it with an unbilled status.
    pub(super) fn note_retry(&mut self, v: &Value) {
        debug_assert_eq!(v.get("subtype").and_then(Value::as_str), Some("api_retry"));
        let status = v.get("error_status").and_then(Value::as_u64);
        if !status.is_some_and(|status| UNBILLED_REJECTIONS.contains(&status)) {
            self.refusal_unprovable = true;
        }
    }

    /// What this process showed about spend before the receipt `v` closed it.
    /// Records the receipt, so that no later receipt in the process can be proven unbilled.
    pub(super) fn spend_evidence(&mut self, v: &Value, is_error: bool) -> SpendEvidence {
        debug_assert_eq!(v.get("type").and_then(Value::as_str), Some("result"));
        let flagged = v.get("is_error").and_then(Value::as_bool) == Some(true);
        debug_assert!(
            !flagged || is_error,
            "an explicit error flag fails the result"
        );
        let provable = !std::mem::replace(&mut self.refusal_unprovable, true);
        if self.streamed_usage_seen || self.receipt_usage_seen {
            return SpendEvidence::Seen;
        }
        if provable && flagged && refused_with_zero_receipt(v) {
            SpendEvidence::Rejected
        } else {
            SpendEvidence::Unseen
        }
    }
}

/// A receipt Claude Code writes for a request the API refused: an unbilled
/// status, zero dollars, no models, and every per-turn counter present as zero.
fn refused_with_zero_receipt(v: &Value) -> bool {
    let status = v.get("api_error_status").and_then(Value::as_u64);
    let refused = status.is_some_and(|status| UNBILLED_REJECTIONS.contains(&status));
    let no_models = v
        .get("modelUsage")
        .and_then(Value::as_object)
        .is_some_and(Map::is_empty);
    let Some(usage) = v.get("usage").and_then(Value::as_object) else {
        return false;
    };
    let counted = ["input_tokens", "output_tokens"]
        .iter()
        .all(|key| usage.get(*key).and_then(Value::as_u64) == Some(0));
    refused
        && no_models
        && counted
        && v.get("total_cost_usd").and_then(Value::as_f64) == Some(0.0)
        && counters_zero(usage)
}

/// Every number is zero, every list is empty, and every field named like a
/// counter is a number. Depth is bounded by serde_json's recursion limit.
fn counters_zero(fields: &Map<String, Value>) -> bool {
    fields.iter().all(|(key, value)| match value {
        Value::Number(number) => number.as_u64() == Some(0),
        Value::Object(inner) => counters_zero(inner),
        Value::Array(items) => items.is_empty(),
        _ => !key.ends_with("_tokens") && !key.ends_with("_requests"),
    })
}
