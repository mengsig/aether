//! The run hard stop: past `run_hard_stop_usd` or `run_hard_stop_tokens` a
//! run's live agents are interrupted and none of its turns is admitted. The
//! agents are not failed, so raising the limit lets them carry on.
use crate::{Result, Runtime};
use aether_core::{
    agent::Agent,
    budget::{HardStop, StopCrossing, average_turn_usage, run_usage},
    event::{Actor, EventDraft, EventKind},
    id::RunId,
    model::Usage,
};
use aether_store::{AgentFilter, Commit, EventFilter};
use std::{collections::BTreeMap, sync::Arc};

/// What one pass knows about a run that is under its stop: the spend on
/// record, what a turn of it costs on average, and the turns running or
/// already started this pass.
struct RunReserve {
    spent: Usage,
    turn: Usage,
    turns: usize,
}

/// Turn admission against the hard stop. `enforce` interrupts runs past it;
/// dispatch then asks `try_start` for each turn it actually selects, so only
/// a turn that starts reserves anything.
pub(super) struct Reserve {
    stop: HardStop,
    runs: BTreeMap<RunId, RunReserve>,
    refused: BTreeMap<RunId, StopCrossing>,
    seen: Vec<RunId>,
}

impl Reserve {
    /// True and reserves the run's average turn when the turn fits under the
    /// stop; false, remembering why, when it would cross it.
    pub(super) fn try_start(&mut self, run: RunId) -> bool {
        if self.refused.contains_key(&run) {
            return false;
        }
        let Some(state) = self.runs.get_mut(&run) else {
            return true;
        };
        let mut reserve = Usage::default();
        for _ in 0..=state.turns {
            reserve += state.turn;
        }
        if let Some(crossing) = self.stop.crossing_with(&state.spent, &reserve) {
            self.refused.insert(run, crossing);
            return false;
        }
        state.turns += 1;
        true
    }

    /// Say which runs are held, and forget the ones that no longer are.
    pub(super) fn finish(self, runtime: &Runtime) {
        for run in &self.seen {
            match self.refused.get(run) {
                Some(crossing) => {
                    runtime.report_problem(problem_key(*run), refusal_message(*run, crossing))
                }
                None => runtime.clear_problem(&problem_key(*run)),
            }
        }
    }
}

/// Interrupt the running turns of every run past its stop, and set up the
/// admission of the rest. With no stop configured nothing is read or changed.
pub(super) fn enforce_hard_stops(runtime: &Arc<Runtime>) -> Result<Reserve> {
    let stop = runtime.config.run_hard_stop;
    let mut reserve = Reserve {
        stop,
        runs: BTreeMap::new(),
        refused: BTreeMap::new(),
        seen: Vec::new(),
    };
    if !stop.is_set() {
        return Ok(reserve);
    }
    let agents = runtime.store.agents(&AgentFilter::default())?;
    let mut by_run: BTreeMap<RunId, Vec<&Agent>> = BTreeMap::new();
    for agent in &agents {
        by_run.entry(agent.run).or_default().push(agent);
    }
    for (run, members) in &by_run {
        if !members.iter().any(|a| a.is_live()) {
            runtime.clear_problem(&problem_key(*run));
            continue;
        }
        reserve.seen.push(*run);
        let spent = run_usage(members.iter().copied());
        if let Some(crossing) = stop.crossing(&spent) {
            interrupt_run(runtime, *run, members, &spent, &crossing)?;
            reserve.refused.insert(*run, crossing);
            continue;
        }
        let turns = running_turns(runtime, members);
        let turn = average_turn_usage(members.iter().copied());
        reserve.runs.insert(*run, RunReserve { spent, turn, turns });
    }
    Ok(reserve)
}

fn running_turns(runtime: &Runtime, members: &[&Agent]) -> usize {
    let sched = runtime.sched.lock();
    members
        .iter()
        .filter(|a| sched.running.contains_key(&a.id))
        .count()
}

fn problem_key(run: RunId) -> String {
    format!("hard_stop.{run}")
}

/// The refusal a person reads: what was crossed and how to lift it.
fn refusal_message(run: RunId, crossing: &StopCrossing) -> String {
    format!(
        "run {run} is held at its hard stop: {} (`[scheduler] {}`). No turn of this run \
         starts. Raise `run_hard_stop_usd` or `run_hard_stop_tokens` in config.toml and \
         restart the daemon to let its agents carry on.",
        crossing.reason, crossing.limit
    )
}

/// Interrupt every running turn in the run, and record the crossing once per
/// limit value: an event for this limit already on the run means it was said.
fn interrupt_run(
    runtime: &Arc<Runtime>,
    run: RunId,
    members: &[&Agent],
    spent: &Usage,
    crossing: &StopCrossing,
) -> Result<()> {
    let interrupted = {
        let sched = runtime.sched.lock();
        let tokens: Vec<_> = members
            .iter()
            .filter_map(|a| sched.running.get(&a.id))
            .map(|r| r.interrupt.clone())
            .collect();
        for token in &tokens {
            token.cancel();
        }
        tokens.len() as u32
    };
    if already_recorded(runtime, run, &crossing.limit)? {
        return Ok(());
    }
    let Some(root) = members
        .iter()
        .find(|a| a.parent.is_none())
        .or(members.first())
    else {
        return Err(crate::RuntimeError::Invalid(format!(
            "run {run} crossed its hard stop but has no agents"
        )));
    };
    tracing::warn!(%run, reason = %crossing.reason, "run hard stop; interrupting its agents");
    let event = EventDraft::new(
        Actor::Scheduler,
        EventKind::RunHardStop {
            reason: crossing.reason.clone(),
            limit: crossing.limit.clone(),
            usage: *spent,
            interrupted,
        },
    )
    .agent(root.id)
    .run(run);
    runtime.apply(Commit::new().event(event))?;
    Ok(())
}

fn already_recorded(runtime: &Runtime, run: RunId, limit: &str) -> Result<bool> {
    let events = runtime.store.latest_events(
        8,
        &EventFilter {
            types: vec!["run.hard_stop".into()],
            ..EventFilter::for_run(run)
        },
    )?;
    Ok(events
        .iter()
        .any(|e| matches!(&e.kind, EventKind::RunHardStop { limit: seen, .. } if seen == limit)))
}

#[cfg(test)]
mod tests;
