//! Native engine accounting.
use super::{
    AgentFilter, AgentId, Arc, Commit, Crossed, EventKind, InvocationSnapshot, ModelCost,
    ModelPrice, Mutation, ResolvedModel, Result, Runtime, RuntimeError, TurnCtx, Usage, json,
};

/// Recompute the tail layer's token count and the snapshot's total from
/// `snapshot.messages`, the source of truth. Called once when the snapshot
/// is first built, and again after [`contract::resend_everything`] rewrites
/// the user turn - the earlier tail/token_estimate were sized to the trimmed
/// prompt and would under-report what the retry actually sends.
pub(super) fn recompute_snapshot_accounting(snapshot: &mut InvocationSnapshot) {
    let estimator = aether_context::estimate::TokenEstimator;
    for layer in &mut snapshot.layers {
        if layer.kind == aether_core::snapshot::LayerKind::RecentTail {
            layer.tokens = estimator.messages(&snapshot.messages);
            layer.items = snapshot.messages.len() as u32;
        }
    }
    snapshot.token_estimate = snapshot.layers.iter().map(|layer| layer.tokens).sum();
}

pub(super) fn record_context_usage(
    runtime: &Runtime,
    ctx: &mut TurnCtx,
    tokens: u64,
) -> Result<()> {
    ctx.agent.stats.context_tokens = Some(tokens);
    let mut commit = Commit::new().event(ctx.draft(EventKind::Custom {
        name: "context.usage".into(),
        payload: json!({"tokens": tokens}),
    }));
    let hold = runtime.hold();
    commit.push_mutation(Mutation::UpsertAgent(crate::turn::agent_row(
        runtime, ctx, &hold,
    )?));
    runtime.apply_held(hold, commit)?;
    Ok(())
}

/// How one engine turn is priced: the registry's price for the model where it
/// has one, and otherwise the list price of the API model behind the
/// subscription, which can only ever produce an *estimate*. Never both - a
/// priced invocation needs no estimate - which is what
/// [`aether_model::Registry::api_equivalent_price`] guarantees.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TurnPricing {
    pub(super) billed: Option<ModelCost>,
    pub(super) estimate: Option<ModelCost>,
}

impl TurnPricing {
    pub(crate) fn of(runtime: &Runtime, resolved: &ResolvedModel) -> Self {
        debug_assert!(
            !resolved.model.id.is_empty(),
            "pricing needs a resolved model"
        );
        let pricing = Self {
            billed: resolved.model.capabilities.cost,
            estimate: runtime.models.registry().api_equivalent_price(resolved),
        };
        debug_assert!(
            pricing.billed.is_none() || pricing.estimate.is_none(),
            "a model with its own price must not also be estimated"
        );
        pricing
    }

    /// What the model an engine session reported costs, for a configured model
    /// nobody could price - `codex/default`, `claude-code/default`, where the
    /// CLI picks. `None` keeps the pricing the turn started with: an
    /// unknown session model is left unpriced rather than given a price.
    pub(super) fn of_session(
        runtime: &Arc<Runtime>,
        configured: &ResolvedModel,
        reported: &str,
    ) -> Option<Self> {
        let pricing = match runtime
            .models
            .registry()
            .session_model_price(configured, reported)?
        {
            ModelPrice::Billed(cost) => Self {
                billed: Some(cost),
                estimate: None,
            },
            ModelPrice::ApiEquivalent(cost) => Self {
                billed: None,
                estimate: Some(cost),
            },
        };
        debug_assert!(
            pricing.billed.is_none() || pricing.estimate.is_none(),
            "a model with its own price must not also be estimated"
        );
        Some(pricing)
    }

    /// Price one invocation's tokens: billed dollars or an estimate, whichever
    /// this model supports.
    pub(crate) fn apply(&self, usage: &mut Usage) {
        usage.price_from(self.billed, self.estimate);
    }

    /// Charge the change in complete-request prices, preserving their cost provenance.
    pub(super) fn request_delta(&self, request: &aether_engine::RequestUsage) -> Usage {
        let coverage = request.delta();
        let mut previous = request.previous;
        let mut total = request.total;
        self.apply(&mut previous);
        self.apply(&mut total);
        let mut usage = Usage {
            cost_usd: total.cost_usd - previous.cost_usd,
            cost_complete: total.cost_complete,
            cost_gap: total.cost_gap,
            cost_basis: total.cost_basis,
            estimated_cost_usd: total
                .estimated_cost_usd
                .zip(previous.estimated_cost_usd)
                .map(|(total, previous)| total - previous),
            ..coverage
        };
        if !coverage.cost_complete {
            usage.set_cost(None);
            usage.cost_gap = coverage.cost_gap;
        }
        usage
    }

    /// The API-equivalent estimate for these counters, where one is available.
    pub(super) fn estimate_of(&self, usage: &Usage) -> Option<f64> {
        self.estimate.map(|rate| rate.cost_of(usage))
    }

    /// Keep request estimates, pricing extra closing counters at base rates as a floor.
    /// Those counters have no request boundaries from which to select a prompt tier.
    pub(super) fn request_estimate_of(&self, total: &Usage, streamed: &Usage) -> Option<f64> {
        let rate = ModelCost {
            above_threshold: None,
            ..self.estimate?
        };
        let extra = Usage {
            input_tokens: total.input_tokens.saturating_sub(streamed.input_tokens),
            output_tokens: total.output_tokens.saturating_sub(streamed.output_tokens),
            cache_read_tokens: total
                .cache_read_tokens
                .saturating_sub(streamed.cache_read_tokens),
            cache_write_tokens: total
                .cache_write_tokens
                .saturating_sub(streamed.cache_write_tokens),
            ..Usage::default()
        };
        Some(streamed.estimated_cost_usd.unwrap_or(0.0) + rate.cost_of(&extra))
    }

    /// Add base-rate pricing for streamed tokens absent from a priced closing receipt.
    /// Missing request boundaries make threshold pricing unavailable.
    pub(super) fn price_closing_shortfall(&self, total: &mut Usage, closing: &Usage) {
        use aether_core::model::CostBasis;
        debug_assert!(self.billed.is_none() || self.estimate.is_none());
        debug_assert!(total.cost_complete && total.cost_basis == CostBasis::ProviderReported);
        debug_assert!(
            total.input_tokens >= closing.input_tokens
                && total.output_tokens >= closing.output_tokens
                && total.cache_read_tokens >= closing.cache_read_tokens
                && total.cache_write_tokens >= closing.cache_write_tokens
                && total.reasoning_tokens.unwrap_or(0) >= closing.reasoning_tokens.unwrap_or(0),
            "the raised closing counters must cover the receipt"
        );
        let mut shortfall = Usage {
            input_tokens: total.input_tokens.saturating_sub(closing.input_tokens),
            output_tokens: total.output_tokens.saturating_sub(closing.output_tokens),
            cache_read_tokens: total
                .cache_read_tokens
                .saturating_sub(closing.cache_read_tokens),
            cache_write_tokens: total
                .cache_write_tokens
                .saturating_sub(closing.cache_write_tokens),
            ..Usage::default()
        };
        if shortfall.total_tokens() == 0 {
            return;
        }
        // Missing request boundaries force base rates, ignoring above_threshold.
        // Billed costs are therefore a lower bound for a tier-crossing request.
        let base_rate = |rate: ModelCost| ModelCost {
            above_threshold: None,
            ..rate
        };
        shortfall.price_from(self.billed.map(base_rate), self.estimate.map(base_rate));
        total.cost_usd += shortfall.cost_usd;
        total.merge_cost_coverage(shortfall);
        total.estimated_cost_usd = shortfall.estimated_cost_usd;
        if self.billed.is_some() || self.estimate.is_some() {
            total.cost_basis = total.cost_basis.combine(shortfall.cost_basis);
        }
    }
}

/// Raise every token field of `usage` to `floor` where `floor` is higher.
/// True when anything had to be raised - the engine reported less than it
/// already had.
pub(super) fn raise_to(usage: &mut Usage, floor: Usage) -> bool {
    let before = *usage;
    usage.input_tokens = usage.input_tokens.max(floor.input_tokens);
    usage.output_tokens = usage.output_tokens.max(floor.output_tokens);
    usage.cache_read_tokens = usage.cache_read_tokens.max(floor.cache_read_tokens);
    usage.cache_write_tokens = usage.cache_write_tokens.max(floor.cache_write_tokens);
    usage.reasoning_tokens = match (usage.reasoning_tokens, floor.reasoning_tokens) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0).max(b.unwrap_or(0))),
    };
    before.total_tokens() != usage.total_tokens()
        || before.reasoning_tokens.unwrap_or(0) != usage.reasoning_tokens.unwrap_or(0)
}

/// What this agent's descendants have spent, cached for the turn. Reading it
/// costs a pass over every agent the store holds, so it is refreshed only when
/// someone else has committed since - this turn's own writes move this agent's
/// row alone and are discounted. Keyed on the raw commit counter the cache
/// never hit at all: every accounting commits this agent's row immediately
/// before the check, so a supervisor paid a whole-fleet roll-up per message.
#[derive(Default)]
pub(super) struct SubtreeUsage {
    /// Commits this turn's own event handling made.
    pub(super) own: u64,
    pub(super) read_at: Option<u64>,
    pub(super) descendants: Usage,
}

impl SubtreeUsage {
    pub(super) fn note_own_commits(&mut self, n: u64) {
        self.own = self.own.saturating_add(n);
    }

    /// The store's commit count with this turn's own writes discounted.
    pub(super) fn key(&self, store_commits: u64) -> u64 {
        store_commits.saturating_sub(self.own)
    }
}

/// The budget this agent has already crossed, if any, and the figure that
/// crossed it. Its own grant covers its subtree, so descendants are rolled
/// up too - but its own running figure is checked first and without touching
/// the store, which is both the common crossing and the hot path. An
/// ancestor's grant stays the scheduler's business (it stops the whole
/// subtree); this only has to keep one engine turn from acting past its own.
pub(super) fn budget_crossed(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    subtree: &mut SubtreeUsage,
) -> Result<Option<Crossed>> {
    if ctx.agent.config.token_budget.is_none() && ctx.agent.config.cost_budget.is_none() {
        return Ok(None);
    }
    // This turn's own figure, which is newer than any stored row.
    let own = ctx.agent.stats.usage;
    if let Some(reason) = ctx.agent.budget_exceeded(&own) {
        return Ok(Some(Crossed { reason, usage: own }));
    }
    let key = subtree.key(runtime.store.commits());
    if subtree.read_at != Some(key) {
        subtree.descendants = descendant_usage(runtime, ctx.agent.id)?;
        subtree.read_at = Some(key);
    }
    let mut total = own;
    total += subtree.descendants;
    Ok(ctx.agent.budget_exceeded(&total).map(|reason| Crossed {
        reason,
        usage: total,
    }))
}

/// Everything this agent's descendants have spent. Terminal descendants count:
/// their spend already happened. Read over every agent rather than this run's,
/// because a child may legally have been created under another run id.
fn descendant_usage(runtime: &Arc<Runtime>, agent: AgentId) -> Result<Usage> {
    // A leaf has no roll-up to read. Indexed on `parent`, so this costs one
    // lookup instead of deserializing every agent the store has ever held.
    if runtime
        .store
        .agents(&AgentFilter {
            parent: Some(agent),
            ..Default::default()
        })?
        .is_empty()
    {
        return Ok(Usage::default());
    }
    let mut agents = runtime.store.agents(&AgentFilter::default())?;
    let Some(row) = agents.iter_mut().find(|a| a.id == agent) else {
        // Fails closed. Enforcement without the agent's own row would quietly
        // stop counting its children, which is how a grant stops binding.
        tracing::error!(%agent, "no stored row while enforcing this agent's budget");
        return Err(RuntimeError::NotFound(format!(
            "agent {agent} while enforcing its budget"
        )));
    };
    // Zeroed so the roll-up is the descendants alone: the caller adds this
    // turn's own figure, which the stored row does not yet have.
    row.stats.usage = Usage::default();
    Ok(aether_core::budget::subtree_usage(&agents)
        .get(&agent)
        .copied()
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::model::{CostBasis, ModelCostTier};

    fn shortfall_rate() -> ModelCost {
        ModelCost {
            input_per_mtok: 1_000_000.0,
            output_per_mtok: 2_000_000.0,
            cache_read_per_mtok: 3_000_000.0,
            cache_write_per_mtok: 4_000_000.0,
            above_threshold: Some(ModelCostTier {
                prompt_tokens: 1,
                input_per_mtok: 10_000_000.0,
                output_per_mtok: 20_000_000.0,
                cache_read_per_mtok: 30_000_000.0,
                cache_write_per_mtok: 40_000_000.0,
            }),
        }
    }

    fn shortfall_counters() -> (Usage, Usage) {
        let closing = Usage {
            input_tokens: 20,
            output_tokens: 2,
            cache_read_tokens: 4,
            cache_write_tokens: 6,
            reasoning_tokens: Some(3),
            cost_usd: 999.0,
            cost_basis: CostBasis::ProviderReported,
            estimated_cost_usd: Some(999.0),
            ..Default::default()
        };
        let raised = Usage {
            input_tokens: 22,
            output_tokens: 6,
            cache_read_tokens: 12,
            cache_write_tokens: 22,
            reasoning_tokens: Some(5),
            cost_usd: 10.0,
            estimated_cost_usd: None,
            ..closing
        };
        (closing, raised)
    }

    #[test]
    fn closing_shortfall_prices_each_token_field_at_base_rates() {
        let (closing, mut raised) = shortfall_counters();
        let original = raised;
        let pricing = TurnPricing {
            billed: Some(shortfall_rate()),
            estimate: None,
        };
        pricing.price_closing_shortfall(&mut raised, &closing);
        assert_eq!(raised.cost_usd, 108.0);
        assert_eq!(raised.cost_basis, CostBasis::Mixed);
        assert!(raised.cost_complete);
        assert_eq!(raised.estimated_cost_usd, None);
        assert_eq!(
            raised,
            Usage {
                cost_usd: 108.0,
                cost_basis: CostBasis::Mixed,
                ..original
            }
        );
    }

    #[test]
    fn closing_shortfall_estimates_only_missing_tokens_at_base_rates() {
        let (closing, mut raised) = shortfall_counters();
        let original = raised;
        let pricing = TurnPricing {
            billed: None,
            estimate: Some(shortfall_rate()),
        };
        pricing.price_closing_shortfall(&mut raised, &closing);
        assert_eq!(raised.cost_usd, 10.0);
        assert!(!raised.cost_complete);
        assert_eq!(raised.cost_basis, CostBasis::Mixed);
        assert_eq!(raised.estimated_cost_usd, Some(98.0));
        assert_eq!(
            raised,
            Usage {
                cost_complete: false,
                cost_gap: Some(aether_core::model::CostGap::Unpriced),
                cost_basis: CostBasis::Mixed,
                estimated_cost_usd: Some(98.0),
                ..original
            }
        );
    }

    #[test]
    fn closing_shortfall_without_rates_keeps_an_incomplete_provider_floor() {
        let (closing, mut raised) = shortfall_counters();
        let original = raised;
        TurnPricing::default().price_closing_shortfall(&mut raised, &closing);
        assert_eq!(raised.cost_usd, 10.0);
        assert_eq!(raised.cost_basis, CostBasis::ProviderReported);
        assert!(!raised.cost_complete);
        assert_eq!(raised.estimated_cost_usd, None);
        assert_eq!(
            raised,
            Usage {
                cost_complete: false,
                cost_gap: Some(aether_core::model::CostGap::Unpriced),
                ..original
            }
        );
    }

    #[test]
    fn closing_shortfall_does_not_bill_reasoning_as_additional_output() {
        let closing = Usage {
            output_tokens: 100,
            cost_usd: 10.0,
            cost_basis: CostBasis::ProviderReported,
            ..Default::default()
        };
        let raised = Usage {
            reasoning_tokens: Some(50),
            ..closing
        };
        for pricing in [
            TurnPricing {
                billed: Some(shortfall_rate()),
                estimate: None,
            },
            TurnPricing {
                billed: None,
                estimate: Some(shortfall_rate()),
            },
            TurnPricing::default(),
        ] {
            let mut total = raised;
            pricing.price_closing_shortfall(&mut total, &closing);
            assert_eq!(total.cost_usd, 10.0);
            assert_eq!(total.output_tokens, 100);
            assert_eq!(total.reasoning_tokens, Some(50));
            assert!(total.cost_complete);
            assert_eq!(total.cost_basis, CostBasis::ProviderReported);
            assert_eq!(total.estimated_cost_usd, None);
            assert_eq!(total, raised);
        }
    }

    #[test]
    fn pricing_cannot_invent_a_reason_for_legacy_incomplete_counters() {
        let pricing = TurnPricing {
            billed: Some(shortfall_rate()),
            estimate: None,
        };
        let request = aether_engine::RequestUsage {
            id: aether_engine::UsageRequestId::Engine("legacy".into()),
            previous: Usage::default(),
            total: serde_json::from_value(serde_json::json!({
                "input_tokens": 10, "output_tokens": 0, "cache_read_tokens": 0,
                "cache_write_tokens": 0, "cost_complete": false
            }))
            .unwrap(),
        };
        let usage = pricing.request_delta(&request);
        assert_eq!(usage.cost_gap, Some(aether_core::model::CostGap::Unknown));
        assert!(!usage.cost_complete);
        assert_eq!(usage.cost_usd, 0.0);
        assert_eq!(usage.input_tokens, 10);
    }
}
