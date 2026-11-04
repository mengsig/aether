//! The guard at scheduler dispatch, and the charge an admitted turn settles.

use std::sync::Arc;

use aether_core::agent::Agent;
use aether_core::id::AgentId;
use aether_core::model::Usage;
use tokio_util::sync::CancellationToken;

use super::{NoesisCharge, NoesisLane, NoesisRefusal};
use crate::Runtime;
use crate::problems::Subject;

/// A Noesis turn the budget admitted, carried into the task that runs it.
pub(crate) struct AdmittedTurn {
    pub(crate) lane: NoesisLane,
    charge: Box<dyn NoesisCharge>,
}

impl AdmittedTurn {
    /// Give the claim back unspent: the scheduler did not start the turn.
    pub(crate) fn release(self) {
        self.charge.settle(&Usage::default());
    }
}

/// What the guard decided for one ready candidate, before the scheduler lock.
pub(crate) enum Dispatch {
    /// Not a Noesis agent: dispatched exactly as before.
    Ordinary,
    Admitted(AdmittedTurn),
    /// No turn this pass: held for a busy cap, parked, or unreadable.
    Withheld,
}

impl Runtime {
    /// Decide one candidate's turn. Store reads and the budget's own locks
    /// happen here, never under `sched`. Only a `startable` one is admitted,
    /// so the budget is not asked for a turn the scheduler cannot start now.
    pub(crate) fn noesis_dispatch(&self, agent: &Agent, startable: bool) -> Dispatch {
        let key = format!("noesis.admission.{}", agent.id);
        let admission = match self.noesis_admission(agent) {
            Ok(found) => {
                self.clear_problem(&key);
                match found {
                    None => return Dispatch::Ordinary,
                    Some(admission) => admission,
                }
            }
            Err(error) => {
                self.report_problem_about(Subject::Agent(agent.id), key, format!(
                    "Agent {} could not be checked against the Noesis gate: {error}; it is withheld and will retry. Run `aether doctor`.",
                    agent.id
                ));
                return Dispatch::Withheld;
            }
        };
        let Some(budget) = admission.budget else {
            self.park_reporting(agent.id, "Noesis is not enabled");
            return Dispatch::Withheld;
        };
        if !startable {
            return Dispatch::Withheld;
        }
        match budget.admit(agent, admission.lane) {
            Ok(charge) => Dispatch::Admitted(AdmittedTurn {
                lane: admission.lane,
                charge,
            }),
            Err(NoesisRefusal::Busy(reason)) => {
                tracing::debug!(agent = %agent.id, %reason, "Noesis cap busy; the turn waits");
                crate::scheduler::hold_after_entry_refusal(self, agent.id);
                Dispatch::Withheld
            }
            Err(NoesisRefusal::Exhausted(reason)) => {
                self.park_reporting(agent.id, &reason);
                Dispatch::Withheld
            }
        }
    }

    /// Park, and keep a failure to park visible: the agent is withheld either way.
    fn park_reporting(&self, id: AgentId, why: &str) {
        if let Err(error) = self.park_noesis(id, why) {
            self.report_problem_about(
                Subject::Agent(id),
                format!("noesis.park.{id}"),
                format!("Noesis agent {id} could not be parked ({why}): {error}; it takes no turn and the park will retry."),
            );
        }
    }
}

/// Run an admitted Noesis turn, then settle its charge with what the turn
/// spent, however the turn ended.
pub(crate) async fn run_admitted(
    runtime: Arc<Runtime>,
    agent: AgentId,
    cancel: CancellationToken,
    interrupt: CancellationToken,
    admitted: AdmittedTurn,
) {
    // Read as the turn starts: the row the scheduler saw may predate a turn
    // that was still winding down.
    let before = runtime.store.agent(agent);
    crate::turn::run(runtime.clone(), agent, cancel, interrupt).await;
    let spent = spent_by_turn(&runtime, agent, before);
    admitted.charge.settle(&spent);
}

/// What a turn spent, from the agent row before it and the row now. A row
/// that cannot be read settles zero, kept visible as a problem.
fn spent_by_turn(
    runtime: &Runtime,
    agent: AgentId,
    before: aether_store::Result<Option<Agent>>,
) -> Usage {
    let read = before.and_then(|before| runtime.store.agent(agent).map(|after| (before, after)));
    let error = match read {
        Ok((Some(before), Some(after))) => {
            debug_assert_eq!(before.id, after.id, "one agent's rows");
            return usage_spent(&before.stats.usage, &after.stats.usage);
        }
        Ok(_) => "the agent row is gone".to_owned(),
        Err(error) => error.to_string(),
    };
    runtime.report_problem_about(Subject::Agent(agent), format!("noesis.charge.{agent}"), format!(
        "Noesis turn of {agent} settled without its spend: {error}. The Noesis budget undercounts this turn."
    ));
    Usage::default()
}

/// What an agent's usage grew by between two readings. Saturating: closing
/// accounting may replace a provisional figure with a smaller one.
pub(crate) fn usage_spent(before: &Usage, after: &Usage) -> Usage {
    debug_assert!(before.cost_usd.is_finite() && after.cost_usd.is_finite());
    debug_assert!(after.cache_write_1h_tokens <= after.cache_write_tokens);
    let estimated = after
        .estimated_cost_usd
        .map(|total| (total - before.estimated_cost_usd.unwrap_or(0.0)).max(0.0));
    Usage {
        input_tokens: after.input_tokens.saturating_sub(before.input_tokens),
        output_tokens: after.output_tokens.saturating_sub(before.output_tokens),
        cache_read_tokens: after
            .cache_read_tokens
            .saturating_sub(before.cache_read_tokens),
        cache_write_tokens: after
            .cache_write_tokens
            .saturating_sub(before.cache_write_tokens),
        cache_write_1h_tokens: after.cache_write_1h_since(before),
        reasoning_tokens: after
            .reasoning_tokens
            .map(|total| total.saturating_sub(before.reasoning_tokens.unwrap_or(0))),
        cost_usd: (after.cost_usd - before.cost_usd).max(0.0),
        cost_complete: after.cost_complete,
        cost_gap: after.cost_gap,
        cost_basis: after.cost_basis,
        estimated_cost_usd: estimated,
    }
}
