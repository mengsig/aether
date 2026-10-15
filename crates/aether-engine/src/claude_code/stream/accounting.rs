//! Retain request high-water marks and attribute streamed tokens to their model.
use super::{EngineEvent, MAX_TRACKED_MESSAGES, ParseState, Usage, UsageRequestId, parse_usage};
use crate::RequestUsage;
use crate::claude_code::session_cost::canonical_model_name;
use serde_json::Value;

impl ParseState {
    pub(super) fn account_usage(
        &mut self,
        key: UsageRequestId,
        value: &Value,
        model: Option<&str>,
    ) -> Vec<EngineEvent> {
        let newly_named = self.track_message(&key, model);
        let total = self.message_usage.entry(key.clone()).or_default();
        let previous = *total;
        *total = high_water(previous, parse_usage(value));
        let request = RequestUsage {
            id: key,
            previous,
            total: *total,
        };
        let delta = request.delta();
        self.record_model_usage(&request, delta, newly_named);
        debug_assert_eq!(self.message_usage.len(), self.usage_order.len());
        debug_assert!(request.total.total_tokens() >= previous.total_tokens());
        let context_tokens = request
            .total
            .input_tokens
            .saturating_add(request.total.cache_read_tokens)
            .saturating_add(request.total.cache_write_tokens);
        let mut events = Vec::new();
        // A split reported after its writes adds no tokens, but reprices the request.
        let reclassified = request.total.cache_write_1h_tokens > previous.cache_write_1h_tokens;
        if delta.total_tokens() > 0 || delta.reasoning_tokens.is_some_and(|n| n > 0) || reclassified
        {
            events.push(EngineEvent::UsageRequest { request });
        }
        events.push(EngineEvent::ContextUsage {
            tokens: context_tokens,
        });
        events
    }

    fn track_message(&mut self, key: &UsageRequestId, model: Option<&str>) -> bool {
        let newly_named = !self.message_models.contains_key(key) && model.is_some();
        if let Some(model) = model {
            self.message_models
                .entry(key.clone())
                .or_insert_with(|| canonical_model_name(model).to_owned());
        }
        if !self.message_usage.contains_key(key) {
            self.usage_order.push_back(key.clone());
        }
        if self.usage_order.len() > MAX_TRACKED_MESSAGES
            && let Some(pos) = self
                .usage_order
                .iter()
                .position(|k| Some(k) != self.streaming_message.as_ref())
            && let Some(evicted) = self.usage_order.remove(pos)
        {
            self.message_usage.remove(&evicted);
            self.message_models.remove(&evicted);
        }
        debug_assert!(self.usage_order.len() <= MAX_TRACKED_MESSAGES);
        debug_assert!(
            self.message_models
                .keys()
                .all(|model_key| model_key == key || self.message_usage.contains_key(model_key))
        );
        newly_named
    }

    fn record_model_usage(&mut self, request: &RequestUsage, delta: Usage, newly_named: bool) {
        debug_assert!(request.total.total_tokens() >= request.previous.total_tokens());
        debug_assert!(request.total.cache_write_1h_tokens <= request.total.cache_write_tokens);
        let Some(model) = self.message_models.get(&request.id) else {
            return;
        };
        let streamed = self.streamed_models.entry(model.clone()).or_default();
        if newly_named {
            *streamed += request.previous;
        }
        let one_hour = streamed.cache_write_1h_tokens.saturating_add(
            request.total.cache_write_1h_tokens - request.previous.cache_write_1h_tokens,
        );
        *streamed += delta;
        streamed.cache_write_1h_tokens = one_hour;
        debug_assert!(streamed.cache_write_1h_tokens <= streamed.cache_write_tokens);
    }
}

fn high_water(previous: Usage, reported: Usage) -> Usage {
    debug_assert!(previous.cache_write_1h_tokens <= previous.cache_write_tokens);
    debug_assert!(reported.cache_write_1h_tokens <= reported.cache_write_tokens);
    Usage {
        input_tokens: previous.input_tokens.max(reported.input_tokens),
        output_tokens: previous.output_tokens.max(reported.output_tokens),
        cache_read_tokens: previous.cache_read_tokens.max(reported.cache_read_tokens),
        cache_write_tokens: previous.cache_write_tokens.max(reported.cache_write_tokens),
        cache_write_1h_tokens: previous
            .cache_write_1h_tokens
            .max(reported.cache_write_1h_tokens),
        reasoning_tokens: match (previous.reasoning_tokens, reported.reasoning_tokens) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0).max(b.unwrap_or(0))),
        },
        ..previous
    }
}
