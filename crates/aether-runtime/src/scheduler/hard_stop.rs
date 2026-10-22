//! The run hard stop: past `run_hard_stop_usd` or `run_hard_stop_tokens` a
//! run's live agents are interrupted and none of its turns is admitted. The
//! agents are not failed, so raising the limit lets them carry on.
use super::Runnable;
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

/// The turns this pass may start: those whose run is under its hard stop,
/// counting what the turn is expected to add. A configured stop crossed by
/// the run's recorded spend also interrupts the run's running turns.
pub(super) fn admit_under_hard_stop(
    runtime: &Arc<Runtime>,
    ready: Vec<Runnable>,
) -> Result<Vec<Runnable>> {
    let stop = runtime.config.run_hard_stop;
    if !stop.is_set() {
        return Ok(ready);
    }
    let agents = runtime.store.agents(&AgentFilter::default())?;
    let mut by_run: BTreeMap<RunId, Vec<&Agent>> = BTreeMap::new();
    for agent in &agents {
        by_run.entry(agent.run).or_default().push(agent);
    }
    let mut refused = BTreeMap::new();
    let mut running_by_run = BTreeMap::new();
    for (run, members) in &by_run {
        let live = members.iter().filter(|a| a.is_live()).count();
        if live == 0 {
            runtime.clear_problem(&problem_key(*run));
            continue;
        }
        let spent = run_usage(members.iter().copied());
        if let Some(crossing) = stop.crossing(&spent) {
            interrupt_run(runtime, *run, members, &spent, &crossing)?;
            refused.insert(*run, crossing);
        } else {
            running_by_run.insert(*run, running_turns(runtime, members));
        }
    }
    let admitted = admit_within_reserve(&stop, &by_run, &mut refused, running_by_run, ready);
    report_refusals(runtime, &by_run, &refused);
    Ok(admitted)
}

/// Admit candidates one at a time, each reserving its own average turn on top
/// of the turns running and the candidates already admitted in this pass.
fn admit_within_reserve(
    stop: &HardStop,
    by_run: &BTreeMap<RunId, Vec<&Agent>>,
    refused: &mut BTreeMap<RunId, StopCrossing>,
    mut starting: BTreeMap<RunId, usize>,
    ready: Vec<Runnable>,
) -> Vec<Runnable> {
    let mut admitted = Vec::new();
    for candidate in ready {
        let run = candidate.agent.run;
        if refused.contains_key(&run) {
            continue;
        }
        let members = &by_run[&run];
        let spent = run_usage(members.iter().copied());
        let slot = starting.entry(run).or_default();
        match reserved_crossing(stop, members, &spent, *slot + 1) {
            Some(crossing) => {
                refused.insert(run, crossing);
            }
            None => {
                *slot += 1;
                admitted.push(candidate);
            }
        }
    }
    admitted
}

fn running_turns(runtime: &Runtime, members: &[&Agent]) -> usize {
    let sched = runtime.sched.lock();
    members
        .iter()
        .filter(|a| sched.running.contains_key(&a.id))
        .count()
}

/// What `turns` more turns (running or about to start) would cross: recorded
/// spend plus the run's average turn for each of them.
fn reserved_crossing(
    stop: &HardStop,
    members: &[&Agent],
    spent: &Usage,
    turns: usize,
) -> Option<StopCrossing> {
    debug_assert!(turns > 0, "admission reserves at least the candidate");
    let turn = average_turn_usage(members.iter().copied());
    let mut reserve = Usage::default();
    for _ in 0..turns {
        reserve += turn;
    }
    stop.crossing_with(spent, &reserve)
}

fn problem_key(run: RunId) -> String {
    format!("hard_stop.{run}")
}

fn report_refusals(
    runtime: &Runtime,
    by_run: &BTreeMap<RunId, Vec<&Agent>>,
    refused: &BTreeMap<RunId, StopCrossing>,
) {
    for run in by_run.keys() {
        match refused.get(run) {
            Some(crossing) => {
                runtime.report_problem(problem_key(*run), refusal_message(*run, crossing))
            }
            None => runtime.clear_problem(&problem_key(*run)),
        }
    }
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
    let root = members
        .iter()
        .find(|a| a.parent.is_none())
        .or(members.first())
        .expect("a run with a live agent has members");
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
