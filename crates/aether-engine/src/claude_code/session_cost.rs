//! Claude reports per-user-turn usage but session-cumulative dollars and modelUsage.

use std::collections::BTreeMap;

use aether_core::agent::EngineSessionUsage;
use aether_core::model::Usage;
use serde_json::Value;

use crate::EngineEvent;

type ModelUsage = BTreeMap<String, Usage>;

#[derive(Debug)]
pub(super) struct SessionCost {
    session: Option<String>,
    before: Option<Usage>,
    before_models: Option<ModelUsage>,
    latest: Option<EngineSessionUsage>,
}

pub(super) struct Receipt {
    pub cost_usd: Option<f64>,
    pub cost_usage: Option<Usage>,
    pub events: Vec<EngineEvent>,
}

impl Default for SessionCost {
    fn default() -> Self {
        Self::resumed(None, Usage::default())
    }
}

impl SessionCost {
    pub(super) fn resumed(session: Option<String>, usage: Usage) -> Self {
        debug_assert!(usage.cost_usd.is_finite() && usage.cost_usd >= 0.0);
        debug_assert!(usage.cache_write_1h_tokens <= usage.cache_write_tokens);
        let before = if session.is_none() {
            Some(Usage::default())
        } else {
            (usage.cost_complete && (usage.cost_usd > 0.0 || usage.total_tokens() > 0))
                .then_some(usage)
        };
        Self {
            session,
            before,
            before_models: None,
            latest: None,
        }
    }

    pub(super) fn with_models(mut self, models: Option<ModelUsage>) -> Self {
        if let (Some(before), Some(models)) = (self.before, models.as_ref()) {
            let sum = sum_counters(models);
            debug_assert!(counters_cover(sum, before) && counters_cover(before, sum));
            debug_assert!(models.values().all(|usage| usage.cost_complete));
        }
        self.before_models = models;
        self
    }

    pub(super) fn receipt(&mut self, value: &Value, streamed: &ModelUsage) -> Receipt {
        let Some(dollars) = value.get("total_cost_usd").and_then(Value::as_f64) else {
            return unknown_receipt(None);
        };
        if !dollars.is_finite() || dollars < 0.0 {
            return unknown_receipt(Some("Claude reported an invalid session cost"));
        }
        let session = value.get("session_id").and_then(Value::as_str);
        let models = model_counters(value);
        let mut total = models.as_ref().map(sum_counters).unwrap_or_default();
        total.set_cost(Some(dollars));
        if !self.accepts(session, total, models.as_ref()) {
            return unknown_receipt(Some(
                "Claude session receipt is zeroed or below its baseline; this invocation's billed cost is unknown",
            ));
        }
        let baseline = self.baseline(session);
        let cost_usd = baseline.map(|before| dollars - before.cost_usd);
        let cost_usage =
            cost_usd.and_then(|_| self.cost_usage(session, models.as_ref()?, streamed));
        let mut receipt = Receipt {
            cost_usd,
            cost_usage,
            events: Vec::new(),
        };
        if cost_usd.is_none() {
            receipt.events.push(EngineEvent::ProtocolNote(
                "Claude session cost baseline is missing; this invocation's billed cost is unknown"
                    .into(),
            ));
        }
        if let Some(session) = session {
            receipt.events.push(self.persist(session, total, models));
        }
        receipt
    }

    fn persist(
        &mut self,
        session: &str,
        mut total: Usage,
        models: Option<ModelUsage>,
    ) -> EngineEvent {
        let before = self.persisted_baseline(Some(session));
        debug_assert!(total.cost_complete);
        debug_assert!(before.is_none_or(|before| total.cost_usd >= before.cost_usd));
        // Missing counters cannot erase an earlier reading or prove token coverage.
        if models.is_none()
            && let Some(before) = before
        {
            let dollars = total.cost_usd;
            total = before;
            total.set_cost(Some(dollars));
        }
        self.latest = Some(EngineSessionUsage {
            session: session.to_owned(),
            total,
            models: models.clone(),
        });
        EngineEvent::SessionUsage {
            session: session.to_owned(),
            total,
            models,
        }
    }

    fn baseline(&self, session: Option<&str>) -> Option<Usage> {
        match (self.session.as_deref(), session) {
            (Some(expected), Some(actual)) if expected != actual => Some(Usage::default()),
            _ => self.before,
        }
    }

    fn persisted_baseline(&self, session: Option<&str>) -> Option<Usage> {
        self.latest
            .as_ref()
            .filter(|latest| Some(latest.session.as_str()) == session)
            .map(|latest| latest.total)
            .or_else(|| self.baseline(session))
    }

    fn accepts(&self, session: Option<&str>, total: Usage, models: Option<&ModelUsage>) -> bool {
        debug_assert!(total.cost_complete);
        debug_assert!(total.cost_usd.is_finite() && total.cost_usd >= 0.0);
        if total.cost_usd == 0.0 && total.total_tokens() == 0 {
            return false;
        }
        let Some(before) = self.persisted_baseline(session) else {
            return true;
        };
        if total.cost_usd < before.cost_usd || (models.is_some() && !counters_cover(total, before))
        {
            return false;
        }
        let prior_models = self
            .latest
            .as_ref()
            .filter(|latest| Some(latest.session.as_str()) == session)
            .and_then(|latest| latest.models.as_ref())
            .or_else(|| {
                (self.session.as_deref() == session)
                    .then_some(self.before_models.as_ref())
                    .flatten()
            });
        match (models, prior_models) {
            (Some(models), Some(prior)) => prior.iter().all(|(model, before)| {
                counters_cover(models.get(model).copied().unwrap_or_default(), *before)
            }),
            _ => true,
        }
    }

    fn cost_usage(
        &self,
        session: Option<&str>,
        models: &ModelUsage,
        streamed: &ModelUsage,
    ) -> Option<Usage> {
        let before = self.baseline(session)?;
        let empty = ModelUsage::new();
        let prior = if before.total_tokens() == 0 && before.cost_usd == 0.0 {
            &empty
        } else {
            self.before_models.as_ref()?
        };
        debug_assert!(!models.is_empty());
        debug_assert!(before.cost_complete);
        let mut coverage = Usage::default();
        for (model, streamed) in streamed {
            let total = models.get(model).copied().unwrap_or_default();
            let before = prior.get(model).copied().unwrap_or_default();
            if !counters_cover(total, before) {
                return None;
            }
            let writes = (total.cache_write_tokens - before.cache_write_tokens)
                .min(streamed.cache_write_tokens);
            coverage += Usage {
                input_tokens: (total.input_tokens - before.input_tokens).min(streamed.input_tokens),
                output_tokens: (total.output_tokens - before.output_tokens)
                    .min(streamed.output_tokens),
                cache_read_tokens: (total.cache_read_tokens - before.cache_read_tokens)
                    .min(streamed.cache_read_tokens),
                cache_write_tokens: writes,
                ..Usage::default()
            };
        }
        Some(coverage)
    }
}

fn unknown_receipt(note: Option<&str>) -> Receipt {
    Receipt {
        cost_usd: None,
        cost_usage: None,
        events: note
            .map(|note| EngineEvent::ProtocolNote(note.into()))
            .into_iter()
            .collect(),
    }
}

fn model_counters(value: &Value) -> Option<ModelUsage> {
    let models = value.get("modelUsage")?.as_object()?;
    if models.is_empty() {
        return None;
    }
    models
        .iter()
        .map(|(name, model)| {
            Some((
                name.clone(),
                Usage {
                    input_tokens: model.get("inputTokens")?.as_u64()?,
                    output_tokens: model.get("outputTokens")?.as_u64()?,
                    cache_read_tokens: model.get("cacheReadInputTokens")?.as_u64()?,
                    cache_write_tokens: model.get("cacheCreationInputTokens")?.as_u64()?,
                    reasoning_tokens: model.get("thinkingTokens").and_then(Value::as_u64),
                    ..Usage::default()
                },
            ))
        })
        .collect()
}

fn sum_counters(models: &ModelUsage) -> Usage {
    models
        .values()
        .copied()
        .fold(Usage::default(), |mut sum, usage| {
            sum += usage;
            sum
        })
}

fn counters_cover(total: Usage, before: Usage) -> bool {
    total.input_tokens >= before.input_tokens
        && total.output_tokens >= before.output_tokens
        && total.cache_read_tokens >= before.cache_read_tokens
        && total.cache_write_tokens >= before.cache_write_tokens
}

#[cfg(test)]
#[path = "tests/session_cost.rs"]
mod tests;
