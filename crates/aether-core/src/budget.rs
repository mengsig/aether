//! Rolling usage up a tree, and what a run is spending.
//!
//! Cost is authoritative on each agent (`stats.usage`); everything above it -
//! a subtree total, a run's rate, a forecast - is derived here so the
//! scheduler, the daemon's projections and every client agree on one answer.

use std::collections::{BTreeMap, BTreeSet};

use crate::agent::Agent;
use crate::id::{AgentId, RunId};
use crate::model::Usage;
use crate::time::Timestamp;
use serde::{Deserialize, Serialize};

/// Usage of each agent plus every descendant of it.
pub fn subtree_usage(agents: &[Agent]) -> BTreeMap<AgentId, Usage> {
    let parents: BTreeMap<AgentId, Option<AgentId>> =
        agents.iter().map(|a| (a.id, a.parent)).collect();
    let mut totals: BTreeMap<AgentId, Usage> =
        agents.iter().map(|a| (a.id, a.stats.usage)).collect();
    for agent in agents {
        let mut seen = BTreeSet::from([agent.id]);
        let mut cursor = agent.parent;
        while let Some(ancestor) = cursor {
            if !seen.insert(ancestor) {
                break; // never trust a cycle in stored links
            }
            let Some(total) = totals.get_mut(&ancestor) else {
                break;
            };
            *total += agent.stats.usage;
            cursor = parents.get(&ancestor).copied().flatten();
        }
    }
    totals
}

/// How far back a run's rate looks: long enough to smooth one turn's burst,
/// short enough that a run gone idle stops forecasting.
pub const RATE_WINDOW_MINUTES: i64 = 15;

/// What one run has spent and where that rate leads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RunSummary {
    pub run: RunId,
    pub agents: u32,
    pub agents_live: u32,
    pub usage: Usage,
    pub started_at: Timestamp,
    /// Dollars per hour over the last 15 minutes (`RATE_WINDOW_MINUTES`) of
    /// receipts, zero for a run that billed nothing in them. Derived from
    /// `usage.cost_usd`, so it is a floor exactly when that is.
    pub rate_usd_per_hour: f64,
    /// What it would have spent after another hour at that rate.
    pub forecast_usd_next_hour: f64,
}

/// What a run has spent: its agents' own usage, terminal ones included, never
/// `subtree_usage` (which would count a child under every ancestor). The one
/// reduction `active_runs` and the scheduler's run budget both read, so a
/// refused delegation and the run a projection reports can never disagree.
pub fn run_usage<'a>(agents: impl IntoIterator<Item = &'a Agent>) -> Usage {
    agents.into_iter().fold(Usage::default(), |mut total, a| {
        total += a.stats.usage;
        total
    })
}

/// A run's hard spend stop: past it the run's live agents are interrupted and
/// no new turn is admitted. Either limit alone is enough; tokens exist for
/// engines that report no dollar figure.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct HardStop {
    pub usd: Option<f64>,
    pub tokens: Option<u64>,
}

/// Which limit a run's spend reached, and the figure it was measured at.
#[derive(Debug, Clone, PartialEq)]
pub struct StopCrossing {
    pub reason: String,
    pub limit: String,
}

impl HardStop {
    pub fn is_set(&self) -> bool {
        self.usd.is_some() || self.tokens.is_some()
    }

    /// The limit `spent` has reached, if any. Dollars are checked first; they
    /// are a floor when part of the run is unpriced, so a crossing is certain.
    pub fn crossing(&self, spent: &Usage) -> Option<StopCrossing> {
        if let Some(limit) = self.usd.filter(|limit| spent.cost_usd >= *limit) {
            return Some(StopCrossing {
                reason: format!(
                    "run spend {} reached its ${limit} hard stop",
                    spent.cost_line()
                ),
                limit: format!("run_hard_stop_usd = {limit}"),
            });
        }
        let limit = self.tokens.filter(|limit| spent.total_tokens() >= *limit)?;
        Some(StopCrossing {
            reason: format!(
                "run token use {} reached its {limit} token hard stop",
                spent.total_tokens()
            ),
            limit: format!("run_hard_stop_tokens = {limit}"),
        })
    }

    /// The limit `spent` plus `reserve` would reach: what turn admission
    /// weighs, so a turn is refused before it is paid for.
    pub fn crossing_with(&self, spent: &Usage, reserve: &Usage) -> Option<StopCrossing> {
        let mut projected = *spent;
        projected.cost_usd += reserve.cost_usd;
        projected.input_tokens = projected.input_tokens.saturating_add(reserve.input_tokens);
        projected.output_tokens = projected
            .output_tokens
            .saturating_add(reserve.output_tokens);
        projected.cache_read_tokens = projected
            .cache_read_tokens
            .saturating_add(reserve.cache_read_tokens);
        projected.cache_write_tokens = projected
            .cache_write_tokens
            .saturating_add(reserve.cache_write_tokens);
        self.crossing(&projected)
    }
}

/// What one more turn of a run is expected to spend: the average of the run's
/// finished turns. Zero for a run with none, which has nothing to estimate from.
pub fn average_turn_usage<'a>(agents: impl IntoIterator<Item = &'a Agent>) -> Usage {
    let (mut total, mut turns) = (Usage::default(), 0u64);
    for agent in agents {
        total += agent.stats.usage;
        turns += u64::from(agent.stats.turns);
    }
    if turns == 0 {
        return Usage::default();
    }
    Usage {
        cost_usd: total.cost_usd / turns as f64,
        input_tokens: total.input_tokens / turns,
        output_tokens: total.output_tokens / turns,
        cache_read_tokens: total.cache_read_tokens / turns,
        cache_write_tokens: total.cache_write_tokens / turns,
        ..Usage::default()
    }
}

/// One summary per run that still has a live agent, oldest run first.
///
/// `window_start` holds, for each agent with a receipt in the last
/// `RATE_WINDOW_MINUTES`, its billed dollars when the window opened; an agent
/// absent from it billed nothing since, so a run with none is idle.
pub fn active_runs(
    agents: &[Agent],
    window_start: &BTreeMap<AgentId, f64>,
    now: Timestamp,
) -> Vec<RunSummary> {
    let mut by_run: BTreeMap<RunId, Vec<&Agent>> = BTreeMap::new();
    for agent in agents {
        by_run.entry(agent.run).or_default().push(agent);
    }
    by_run
        .into_iter()
        .filter(|(_, members)| members.iter().any(|a| a.is_live()))
        .map(|(run, members)| {
            let usage = run_usage(members.iter().copied());
            let started_at = members.iter().map(|a| a.created_at).min().unwrap_or(now);
            // The floor, like everything else derived from a cost: a run with
            // an unpriced agent forecasts what it is known to be spending,
            // marked by `usage.cost_complete`, never a confident zero.
            let rate = recent_rate(&members, window_start, started_at, now);
            debug_assert!(rate >= 0.0, "a rate of spend is never negative");
            RunSummary {
                run,
                agents: members.len() as u32,
                agents_live: members.iter().filter(|a| a.is_live()).count() as u32,
                usage,
                started_at,
                rate_usd_per_hour: rate,
                forecast_usd_next_hour: usage.cost_usd + rate,
            }
        })
        .collect()
}

/// Dollars per hour the run's agents billed inside the window, over the part
/// of the window the run has existed for.
fn recent_rate(
    members: &[&Agent],
    window_start: &BTreeMap<AgentId, f64>,
    started_at: Timestamp,
    now: Timestamp,
) -> f64 {
    let spent: f64 = members
        .iter()
        .filter_map(|a| {
            let before = window_start.get(&a.id)?;
            debug_assert!(*before >= 0.0, "a cumulative cost is never negative");
            // Cumulative cost only grows; a reading above it is a stale row.
            Some((a.stats.usage.cost_usd - before).max(0.0))
        })
        .sum();
    let window = chrono::Duration::minutes(RATE_WINDOW_MINUTES);
    let observed = (now - started_at).clamp(chrono::Duration::zero(), window);
    // Below a minute the rate is noise, not a forecast.
    if observed < chrono::Duration::minutes(1) {
        return 0.0;
    }
    spent / (observed.num_milliseconds() as f64 / 3_600_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentConfig, AgentStats, AgentStatus, Role, Scope};
    use crate::model::ModelRef;
    use crate::time::now;

    fn agent(parent: Option<AgentId>, run: RunId, cost: f64, live: bool) -> Agent {
        let at = now();
        Agent {
            id: AgentId::new(),
            parent,
            role: Role::new("w"),
            name: "w".into(),
            scope: Scope::default(),
            model: ModelRef::new("mock", "m"),
            fallbacks: Vec::new(),
            status: if live {
                AgentStatus::Ready
            } else {
                AgentStatus::Completed { summary: None }
            },
            current_task: None,
            inbox_cursor: 0,
            checkpoint: None,
            engine_session: None,
            engine_usage: None,
            engine_sent: None,
            status_since: None,
            archived: false,
            paused: false,
            paused_via: None,
            resume_wait: None,
            restarted_from: None,
            restarted_as: None,
            config: AgentConfig::default(),
            stats: AgentStats {
                usage: Usage {
                    input_tokens: 10,
                    cost_usd: cost,
                    ..Default::default()
                },
                ..Default::default()
            },
            run,
            created_at: at,
            updated_at: at,
        }
    }

    #[test]
    fn usage_rolls_up_through_every_ancestor() {
        let run = RunId::new();
        let root = agent(None, run, 1.0, true);
        let mid = agent(Some(root.id), run, 2.0, true);
        let leaf = agent(Some(mid.id), run, 4.0, false);
        let totals = subtree_usage(&[root.clone(), mid.clone(), leaf.clone()]);
        assert_eq!(totals[&leaf.id].cost_usd, 4.0);
        assert_eq!(totals[&mid.id].cost_usd, 6.0);
        assert_eq!(totals[&root.id].cost_usd, 7.0);
        assert!(totals[&root.id].cost_complete);
        assert_eq!(totals[&root.id].input_tokens, 30);
    }

    /// A run of subscription agents carries its estimate up the tree for a
    /// reader, and nothing else: the rate, the forecast and the cost a budget
    /// reads are all billed dollars only.
    #[test]
    fn an_estimate_rolls_up_but_never_becomes_spend() {
        let run = RunId::new();
        let mut root = agent(None, run, 0.0, true);
        root.stats.usage.cost_complete = false;
        root.stats.usage.estimated_cost_usd = Some(1.36);
        root.created_at = now() - chrono::Duration::minutes(30);
        let mut child = agent(Some(root.id), run, 0.0, false);
        child.stats.usage.cost_complete = false;
        child.stats.usage.estimated_cost_usd = Some(0.64);

        let totals = subtree_usage(&[root.clone(), child.clone()]);
        assert_eq!(totals[&root.id].estimated_cost_usd, Some(2.0));
        assert_eq!(totals[&root.id].cost_usd, 0.0);
        assert_eq!(totals[&root.id].api_equivalent_usd(), Some(2.0));

        let total = run_usage([&root, &child]);
        assert_eq!(total.cost_usd, 0.0, "nobody billed this run");
        assert_eq!(total.estimated_cost_usd, Some(2.0));

        let billing = BTreeMap::from([(root.id, 0.0), (child.id, 0.0)]);
        let summary = &active_runs(&[root, child], &billing, now())[0];
        assert_eq!(summary.usage.estimated_cost_usd, Some(2.0));
        assert_eq!(
            summary.rate_usd_per_hour, 0.0,
            "an estimate is not a rate of spend"
        );
        assert_eq!(summary.forecast_usd_next_hour, 0.0);
    }

    #[test]
    fn a_run_without_a_live_agent_is_not_forecast() {
        let live = RunId::new();
        let finished = RunId::new();
        let runs = active_runs(
            &[
                agent(None, live, 1.0, true),
                agent(None, finished, 9.0, false),
            ],
            &BTreeMap::new(),
            now(),
        );
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run, live);
        assert_eq!(runs[0].agents_live, 1);
    }

    /// Only what was billed inside the window counts: $4 of the $6 came in
    /// the last 15 minutes, so the run burns $16/h, not its lifetime $4/h.
    #[test]
    fn the_forecast_follows_the_recent_rate_of_spend() {
        let at = now();
        let run = RunId::new();
        let mut a = agent(None, run, 6.0, true);
        a.created_at = at - chrono::Duration::minutes(90);
        let summary = &active_runs(std::slice::from_ref(&a), &BTreeMap::from([(a.id, 2.0)]), at)[0];
        assert!(
            (summary.rate_usd_per_hour - 16.0).abs() < 1e-9,
            "{summary:?}"
        );
        assert!((summary.forecast_usd_next_hour - 22.0).abs() < 1e-9);
    }

    /// A run whose last receipt is older than the window is idle: however
    /// much it spent before, it forecasts no further spend.
    #[test]
    fn a_run_without_a_receipt_in_the_window_is_idle() {
        let at = now();
        let run = RunId::new();
        let mut a = agent(None, run, 843.18, true);
        a.created_at = at - chrono::Duration::hours(5);
        a.stats.last_turn_at = Some(at - chrono::Duration::minutes(RATE_WINDOW_MINUTES + 1));
        let summary = &active_runs(&[a], &BTreeMap::new(), at)[0];
        assert_eq!(summary.rate_usd_per_hour, 0.0);
        assert_eq!(summary.forecast_usd_next_hour, 843.18);
    }

    /// A run younger than the window is rated over its own age.
    #[test]
    fn a_young_run_is_rated_over_its_age() {
        let at = now();
        let run = RunId::new();
        let mut a = agent(None, run, 1.0, true);
        a.created_at = at - chrono::Duration::minutes(5);
        let summary = &active_runs(std::slice::from_ref(&a), &BTreeMap::from([(a.id, 0.0)]), at)[0];
        assert!(
            (summary.rate_usd_per_hour - 12.0).abs() < 1e-9,
            "{summary:?}"
        );
    }

    #[test]
    fn a_run_younger_than_a_minute_has_no_rate() {
        let run = RunId::new();
        let a = agent(None, run, 5.0, true);
        let summary = &active_runs(
            std::slice::from_ref(&a),
            &BTreeMap::from([(a.id, 0.0)]),
            now(),
        )[0];
        assert_eq!(summary.rate_usd_per_hour, 0.0);
        assert_eq!(summary.forecast_usd_next_hour, 5.0);
    }

    #[test]
    fn tiered_request_costs_feed_subtree_limits_and_forecasts() {
        use crate::model::{ModelCost, ModelCostTier};

        let rate = ModelCost {
            input_per_mtok: 0.10,
            above_threshold: Some(ModelCostTier {
                prompt_tokens: 100_000,
                input_per_mtok: 0.50,
                output_per_mtok: 2.50,
                cache_read_per_mtok: 0.05,
                cache_write_per_mtok: 0.625,
                cache_write_1h_per_mtok: None,
            }),
            ..Default::default()
        };
        let at = now();
        let run = RunId::new();
        let mut root = agent(None, run, 0.0, true);
        root.created_at = at - chrono::Duration::minutes(30);
        root.config.cost_budget = Some(0.04);
        root.stats.usage = Usage {
            input_tokens: 80_000,
            ..Default::default()
        };
        root.stats.usage.price_from(Some(rate), None);
        let mut child = agent(Some(root.id), run, 0.0, false);
        child.created_at = root.created_at;
        child.stats.usage = Usage {
            input_tokens: 100_001,
            ..Default::default()
        };
        child.stats.usage.price_from(Some(rate), None);
        let members = [root.clone(), child];
        let totals = subtree_usage(&members);
        assert!((totals[&root.id].cost_usd - 0.0580005).abs() < 1e-12);
        assert!(root.budget_exceeded(&totals[&root.id]).is_some());
        let billing = members.iter().map(|a| (a.id, 0.0)).collect();
        let summary = &active_runs(&members, &billing, at)[0];
        assert!((summary.usage.cost_usd - 0.0580005).abs() < 1e-12);
        assert!((summary.rate_usd_per_hour - 0.232002).abs() < 1e-12);
        assert!((summary.forecast_usd_next_hour - 0.2900025).abs() < 1e-12);
    }

    #[test]
    fn cost_gap_reasons_roll_up_through_budgets_and_run_summaries() {
        use crate::model::CostGap;
        let run = RunId::new();
        let mut root = agent(None, run, 1.0, true);
        let mut child = agent(Some(root.id), run, 2.0, false);
        root.stats.usage.mark_cost_gap(CostGap::Unpriced);
        child.stats.usage.mark_cost_gap(CostGap::Interrupted);
        let members = [root.clone(), child];
        let total = subtree_usage(&members)[&root.id];
        assert_eq!(total.cost_gap, Some(CostGap::Mixed));
        assert_eq!(total.cost_usd, 3.0);
        assert_eq!(total.total_tokens(), 20);
        let summary = &active_runs(&members, &BTreeMap::new(), now())[0];
        assert_eq!(summary.usage, total);
        assert!(!summary.usage.cost_complete);
    }
}
