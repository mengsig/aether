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
    /// Receipts are cumulative per run; after a reset, none can say what the run cost.
    reset: bool,
}

/// What an invocation showed about spend before its closing receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SpendEvidence {
    /// Streamed or per-turn usage shows a model ran.
    Seen,
    /// No usage arrived, but the invocation may still have answered.
    Unseen,
    /// No usage arrived, nothing earlier in the process can have been billed,
    /// and the API refused the request with a status it does not bill.
    Rejected,
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
            reset: false,
        }
    }

    pub(super) fn with_models(mut self, models: Option<ModelUsage>) -> Self {
        let models = models.map(canonical_model_usage);
        if let (Some(before), Some(models)) = (self.before, models.as_ref()) {
            let sum = sum_counters(models);
            debug_assert!(counters_cover(sum, before) && counters_cover(before, sum));
            debug_assert!(models.values().all(|usage| usage.cost_complete));
        }
        self.before_models = models;
        self
    }

    pub(super) fn receipt(
        &mut self,
        value: &Value,
        streamed: &ModelUsage,
        spend: SpendEvidence,
    ) -> Receipt {
        let has_spend = spend == SpendEvidence::Seen;
        let Some(dollars) = value.get("total_cost_usd").and_then(Value::as_f64) else {
            return unknown_receipt(None, has_spend);
        };
        if !dollars.is_finite() || dollars < 0.0 {
            return unknown_receipt(Some("Claude reported an invalid session cost"), has_spend);
        }
        let session = value.get("session_id").and_then(Value::as_str);
        let models = model_counters(value);
        let mut total = models.as_ref().map(sum_counters).unwrap_or_default();
        total.set_cost(Some(dollars));
        if dollars == 0.0 && total.total_tokens() == 0 && total.reasoning_tokens.unwrap_or(0) == 0 {
            if spend == SpendEvidence::Rejected {
                return zero_receipt();
            }
            return unknown_receipt(
                Some("Claude session receipt is zeroed; this invocation's billed cost is unknown"),
                has_spend,
            );
        }
        let accepted = self.accepts(session, total, models.as_ref());
        let baseline = self.baseline(session).filter(|_| accepted && !self.reset);
        let cost_usd = baseline.map(|before| dollars - before.cost_usd);
        let cost_usage =
            cost_usd.and_then(|_| self.cost_usage(session, models.as_ref()?, streamed));
        let mut receipt = Receipt {
            cost_usd,
            cost_usage,
            events: Vec::new(),
        };
        if cost_usd.is_none() {
            receipt = unknown_receipt(
                Some(
                    "Claude session cost baseline is missing or reset; this invocation's billed cost is unknown",
                ),
                has_spend,
            );
        }
        if !accepted {
            self.adopt_reset(session, total, models.clone());
        }
        if let Some(session) = session {
            receipt.events.push(self.persist(session, total, models));
        }
        debug_assert!(receipt.cost_usd.is_none_or(|cost| cost >= 0.0));
        debug_assert!(receipt.cost_usage.is_none() || receipt.cost_usd.is_some());
        receipt
    }

    fn adopt_reset(&mut self, session: Option<&str>, total: Usage, models: Option<ModelUsage>) {
        debug_assert!(total.cost_complete);
        debug_assert!(
            total.cost_usd > 0.0
                || total.total_tokens() > 0
                || total.reasoning_tokens.unwrap_or(0) > 0
        );
        self.session = session.map(str::to_owned);
        self.before = Some(total);
        self.reset = true;
        self.before_models = models;
    }

    fn persist(&mut self, session: &str, total: Usage, models: Option<ModelUsage>) -> EngineEvent {
        debug_assert!(total.cost_complete);
        debug_assert!(total.cost_usd.is_finite() && total.cost_usd >= 0.0);
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
        let Some(before) = self.persisted_baseline(session) else {
            return true;
        };
        if total.cost_usd < before.cost_usd || (models.is_some() && !counters_cover(total, before))
        {
            return false;
        }
        let prior_models = if let Some(latest) = self
            .latest
            .as_ref()
            .filter(|latest| Some(latest.session.as_str()) == session)
        {
            latest.models.as_ref()
        } else {
            (self.session.as_deref() == session)
                .then_some(self.before_models.as_ref())
                .flatten()
        };
        match (models, prior_models) {
            (Some(models), Some(prior)) => prior.iter().all(|(model, before)| {
                models
                    .get(model)
                    .is_some_and(|total| counters_cover(*total, *before))
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

/// A refused request with no usage streamed or counted billed nothing.
fn zero_receipt() -> Receipt {
    Receipt {
        cost_usd: Some(0.0),
        cost_usage: None,
        events: Vec::new(),
    }
}

/// Marks the turn's cost as missing a reading the engine never reported.
pub(super) fn cost_gap() -> EngineEvent {
    let mut usage = Usage::default();
    usage.mark_cost_gap(aether_core::model::CostGap::Unreported);
    EngineEvent::UsageDelta { usage }
}

fn unknown_receipt(note: Option<&str>, has_spend: bool) -> Receipt {
    let mut events = Vec::new();
    if let Some(note) = note {
        events.push(EngineEvent::ProtocolNote(note.into()));
    }
    if has_spend {
        events.push(cost_gap());
    }
    Receipt {
        cost_usd: None,
        cost_usage: None,
        events,
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
        .collect::<Option<ModelUsage>>()
        .map(canonical_model_usage)
}

pub(super) fn canonical_model_name(name: &str) -> &str {
    if let Some((base, date)) = name.rsplit_once('-')
        && base.starts_with("claude-")
        && date.len() == 8
        && date.bytes().all(|byte| byte.is_ascii_digit())
    {
        return base;
    }
    name
}

fn canonical_model_usage(models: ModelUsage) -> ModelUsage {
    models
        .into_iter()
        .fold(ModelUsage::new(), |mut canonical, (model, usage)| {
            *canonical
                .entry(canonical_model_name(&model).to_owned())
                .or_default() += usage;
            canonical
        })
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
        && total.reasoning_tokens.unwrap_or(0) >= before.reasoning_tokens.unwrap_or(0)
}

#[cfg(test)]
#[path = "tests/session_cost.rs"]
mod tests;
