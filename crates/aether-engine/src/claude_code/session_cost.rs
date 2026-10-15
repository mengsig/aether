//! Claude reports per-user-turn usage but session-cumulative dollars and modelUsage.

use aether_core::model::Usage;
use serde_json::Value;

use crate::EngineEvent;

#[derive(Debug)]
pub(super) struct SessionCost {
    session: Option<String>,
    before: Option<Usage>,
}

impl Default for SessionCost {
    fn default() -> Self {
        Self {
            session: None,
            before: Some(Usage::default()),
        }
    }
}

impl SessionCost {
    pub(super) fn resumed(session: Option<String>, usage: Usage) -> Self {
        debug_assert!(usage.cost_usd.is_finite() && usage.cost_usd >= 0.0);
        debug_assert!(usage.cache_write_1h_tokens <= usage.cache_write_tokens);
        let before = if session.is_none() {
            Some(Usage::default())
        } else {
            usage.cost_complete.then_some(usage)
        };
        Self { session, before }
    }

    pub(super) fn receipt(&self, value: &Value) -> (Option<f64>, Vec<EngineEvent>) {
        let Some(total) = value.get("total_cost_usd").and_then(Value::as_f64) else {
            return (None, Vec::new());
        };
        if !total.is_finite() || total < 0.0 {
            return (
                None,
                vec![EngineEvent::ProtocolNote(
                    "Claude reported an invalid session cost".into(),
                )],
            );
        }
        let session = value.get("session_id").and_then(Value::as_str);
        let cost = self
            .baseline(session)
            .filter(|before| total >= before.cost_usd)
            .map(|before| total - before.cost_usd);
        let mut events = Vec::new();
        if cost.is_none() {
            events.push(EngineEvent::ProtocolNote(
                "Claude session cost baseline is missing or exceeds its receipt; this invocation's billed cost is unknown".into(),
            ));
        }
        if let Some(session) = session {
            let mut counters = Usage::default();
            // Unread counters retain only the dollar baseline, never token coverage.
            if let Some(models) = value.get("modelUsage").and_then(Value::as_object)
                && has_model_counters(value)
            {
                for model in models.values() {
                    counters += Usage {
                        input_tokens: number(model, "inputTokens"),
                        output_tokens: number(model, "outputTokens"),
                        cache_read_tokens: number(model, "cacheReadInputTokens"),
                        cache_write_tokens: number(model, "cacheCreationInputTokens"),
                        reasoning_tokens: model.get("thinkingTokens").and_then(Value::as_u64),
                        ..Usage::default()
                    };
                }
            }
            counters.set_cost(Some(total));
            events.push(EngineEvent::SessionUsage {
                session: session.to_owned(),
                total: counters,
            });
        }
        (cost, events)
    }

    fn baseline(&self, session: Option<&str>) -> Option<Usage> {
        match (self.session.as_deref(), session) {
            (Some(expected), Some(actual)) if expected != actual => Some(Usage::default()),
            _ => self.before,
        }
    }

    pub(super) fn cost_usage(&self, value: &Value, total: Usage) -> Option<Usage> {
        debug_assert!(total.cost_complete);
        debug_assert_eq!(
            value.get("total_cost_usd").and_then(Value::as_f64),
            Some(total.cost_usd)
        );
        let before = self.baseline(value.get("session_id").and_then(Value::as_str))?;
        if !has_model_counters(value) || (before.cost_usd > 0.0 && before.total_tokens() == 0) {
            return None;
        }
        if total.input_tokens < before.input_tokens
            || total.output_tokens < before.output_tokens
            || total.cache_read_tokens < before.cache_read_tokens
            || total.cache_write_tokens < before.cache_write_tokens
        {
            return None;
        }
        let usage = Usage {
            input_tokens: total.input_tokens - before.input_tokens,
            output_tokens: total.output_tokens - before.output_tokens,
            cache_read_tokens: total.cache_read_tokens - before.cache_read_tokens,
            cache_write_tokens: total.cache_write_tokens - before.cache_write_tokens,
            ..Usage::default()
        };
        Some(usage)
    }
}

fn has_model_counters(value: &Value) -> bool {
    let Some(models) = value.get("modelUsage").and_then(Value::as_object) else {
        return false;
    };
    let fields = [
        "inputTokens",
        "outputTokens",
        "cacheReadInputTokens",
        "cacheCreationInputTokens",
    ];
    !models.is_empty()
        && models.values().all(|model| {
            fields
                .iter()
                .all(|key| model.get(key).and_then(Value::as_u64).is_some())
        })
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

#[cfg(test)]
#[path = "tests/session_cost.rs"]
mod tests;
