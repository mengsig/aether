//! Prove that a failed invocation billed nothing: the API refused it before
//! inference, and nothing earlier in the process can have been billed.
use serde_json::{Map, Value};

use super::{EngineEvent, ParseState};
use crate::claude_code::session_cost::{SpendEvidence, cost_gap};

/// Statuses the API answers before inference and does not bill. Claude Code
/// reports each retry's status, except a reshaped resend after a 400.
const UNBILLED_REJECTIONS: [u64; 5] = [400, 401, 403, 404, 413];

impl ParseState {
    /// An `api_retry` record: the attempt before it is unbilled only when the
    /// API refused it. The retry is a new attempt, so it withdraws a $0 already proven.
    pub(super) fn note_retry(&mut self, v: &Value) -> Vec<EngineEvent> {
        debug_assert_eq!(v.get("subtype").and_then(Value::as_str), Some("api_retry"));
        debug_assert!(!self.zero_proven || self.refusal_unprovable);
        let status = v.get("error_status").and_then(Value::as_u64);
        if !status.is_some_and(|status| UNBILLED_REJECTIONS.contains(&status)) {
            self.refusal_unprovable = true;
        }
        if std::mem::take(&mut self.zero_proven) {
            return vec![cost_gap()];
        }
        Vec::new()
    }

    /// A `status` record naming work, such as `requesting`, starts a request. One
    /// after a proven $0 is a request the receipt cannot cover, so it withdraws it.
    pub(super) fn note_status(&mut self, v: &Value) -> Vec<EngineEvent> {
        debug_assert_eq!(v.get("subtype").and_then(Value::as_str), Some("status"));
        debug_assert!(!self.zero_proven || self.refusal_unprovable);
        let working = v.get("status").is_some_and(|status| !status.is_null());
        if working && std::mem::take(&mut self.zero_proven) {
            return vec![cost_gap()];
        }
        Vec::new()
    }

    /// Model output, counted or not, shows inference ran; Claude Code's API-error message is not
    /// model output. Output after a proven $0 starts a new request, so it withdraws the $0.
    pub(super) fn note_output(&mut self, v: &Value) -> Vec<EngineEvent> {
        let kind = v.get("type").and_then(Value::as_str);
        debug_assert!(
            matches!(kind, Some("assistant" | "stream_event")),
            "{kind:?}"
        );
        debug_assert!(!self.zero_proven || self.refusal_unprovable);
        let api_error_message = kind == Some("assistant")
            && v.get("is_api_error_message").and_then(Value::as_bool) == Some(true)
            && v.pointer("/message/model").and_then(Value::as_str) == Some("<synthetic>");
        self.refusal_unprovable |= !api_error_message;
        if std::mem::take(&mut self.zero_proven) {
            return vec![cost_gap()];
        }
        Vec::new()
    }

    /// A result that closed queued work without the model still ends a receipt,
    /// so no later receipt in the process is its first.
    pub(super) fn note_skipped_receipt(&mut self) {
        debug_assert!(self.prompt.is_some(), "only a pre-echo result is skipped");
        self.refusal_unprovable = true;
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
            self.zero_proven = true;
            SpendEvidence::Rejected
        } else {
            SpendEvidence::Unseen
        }
    }
}

/// A receipt Claude Code writes for a request the API refused: an unbilled
/// status, zero dollars, no models, and every per-turn counter present as zero.
fn refused_with_zero_receipt(v: &Value) -> bool {
    debug_assert_eq!(v.get("type").and_then(Value::as_str), Some("result"));
    debug_assert_eq!(v.get("is_error").and_then(Value::as_bool), Some(true));
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

/// Every field named like a counter is a zero number, every other number is
/// zero, and every list is empty. Depth is bounded by serde_json's recursion limit.
fn counters_zero(fields: &Map<String, Value>) -> bool {
    fields.iter().all(|(key, value)| {
        if key.ends_with("_tokens") || key.ends_with("_requests") {
            return value.as_u64() == Some(0);
        }
        match value {
            Value::Number(number) => number.as_u64() == Some(0),
            Value::Object(inner) => counters_zero(inner),
            Value::Array(items) => items.is_empty(),
            _ => true,
        }
    })
}
