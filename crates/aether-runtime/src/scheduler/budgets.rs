use crate::{Result, Runtime};
use aether_core::{
    agent::{Agent, AgentStatus},
    event::{Actor, EventDraft, EventKind},
    id::AgentId,
    time::now,
};
use aether_store::{AgentFilter, Commit, EventFilter};
use std::sync::Arc;
/// The roll-up reads only stored state, so nothing but a commit can change its
/// verdict. Skip an unchanged fleet to avoid repeating subtree reads.
pub(super) fn enforce_budgets(runtime: &Arc<Runtime>) -> Result<()> {
    // Read before the pass: a commit landing while it runs was never looked
    // at, and recording a later count would mark it checked and skip it. The
    // cost of sampling early is at most one repeat pass, and the pass's own
    // writes make no second warning (`already_warned`) and stop no agent
    // twice (`still_live`). Left alone on failure, so the next tick retries.
    let watermark = runtime.store.commits();
    if runtime.sched.lock().budgets_checked_after == Some(watermark) {
        return Ok(());
    }
    check_budgets(runtime)?;
    runtime.sched.lock().budgets_checked_after = Some(watermark);
    Ok(())
}

/// VISION §3.10: a budget stops the agent cleanly as Failed with the reason,
/// and warns the person before that. A budget covers the agent and its
/// descendants: a supervisor's limit is meaningless if delegating escapes it.
fn check_budgets(runtime: &Arc<Runtime>) -> Result<()> {
    let budgeted = runtime.store.agents(&AgentFilter {
        live_only: true,
        budgeted_only: true,
        ..Default::default()
    })?;
    for agent in budgeted {
        debug_assert!(agent.is_live());
        debug_assert!(agent.config.token_budget.is_some() || agent.config.cost_budget.is_some());
        // Terminal descendants still count: their spend already happened.
        let mut usage = aether_core::model::Usage::default();
        for id in runtime.store.subtree_ids(agent.id)? {
            if let Some(member) = runtime.store.agent(id)? {
                usage += member.stats.usage;
            }
        }
        let Some(mut agent) = still_live(runtime, agent.id)? else {
            continue;
        };
        // Said before the stop is considered: an operator whose cost budget is
        // enforced on a floor should hear that whether or not the floor is
        // near the limit. Re-read after it, because `stop_over_budget` needs
        // a row read after this pass's last commit.
        if let Some(reason) = agent.cost_floor_note(&usage)
            && !already_warned(runtime, agent.id, Notice::CostFloor)?
        {
            warn_once(runtime, &agent, usage, reason)?;
            let Some(fresh) = still_live(runtime, agent.id)? else {
                continue;
            };
            agent = fresh;
        }
        if let Some(reason) = agent.budget_exceeded(&usage) {
            stop_over_budget(runtime, &agent, usage, reason, Actor::Scheduler)?;
        } else if let Some(reason) = agent.budget_warning(&usage)
            && !already_warned(runtime, agent.id, Notice::SoftLimit)?
        {
            warn_once(runtime, &agent, usage, reason)?;
        }
    }
    Ok(())
}

pub(super) fn warn_once(
    runtime: &Arc<Runtime>,
    agent: &Agent,
    usage: aether_core::model::Usage,
    reason: String,
) -> Result<()> {
    // Re-read under the hold: a stop committed since the caller's read must
    // not be followed by a warning for the same agent.
    let hold = runtime.hold();
    if still_live(runtime, agent.id)?.is_none() {
        return Ok(());
    }
    tracing::info!(agent = %agent.id, %reason, "budget warning");
    let commit = Commit::new().event(
        EventDraft::new(Actor::Scheduler, EventKind::BudgetWarning { reason, usage })
            .agent(agent.id)
            .run(agent.run),
    );
    runtime.apply_held(hold, commit)?;
    Ok(())
}

/// The agent as it stands now, or `None` if it has ended. A pass that loops
/// over a snapshot re-reads through this before acting on a row: stopping a
/// supervisor cancels its subtree, so a descendant read from the snapshot may
/// already have ended. Acting on the stale row would write a second terminal
/// event over a decision that is already committed.
fn still_live(runtime: &Runtime, id: AgentId) -> Result<Option<Agent>> {
    Ok(runtime.store.agent(id)?.filter(|a| a.is_live()))
}

/// VISION §3.1: `deadline` is a hard stop like a budget. The coarse tick is
/// the clock, so an agent stops within a second of its instant.
pub(super) fn enforce_deadlines(runtime: &Arc<Runtime>, live: &[Agent]) -> Result<()> {
    let at = now();
    for agent in live {
        debug_assert!(agent.is_live());
        if agent.deadline_passed(at).is_none() {
            continue;
        }
        debug_assert!(agent.config.deadline.is_some());
        let Some(agent) = still_live(runtime, agent.id)? else {
            continue;
        };
        let Some(reason) = agent.deadline_passed(at) else {
            continue;
        };
        tracing::warn!(agent = %agent.id, %reason, "deadline passed; stopping the agent");
        let mut commit = Commit::new().event(
            EventDraft::new(
                Actor::Scheduler,
                EventKind::DeadlinePassed {
                    reason: reason.clone(),
                },
            )
            .agent(agent.id)
            .run(agent.run),
        );
        stop_and_cancel_subtree(runtime, &agent, &mut commit, reason)?;
    }
    Ok(())
}

/// What a budget warning is about. The two are said once each: a soft limit
/// repeated every tick is noise, and the lower-bound notice is not the soft
/// limit, so one must not suppress the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Notice {
    SoftLimit,
    CostFloor,
}

impl Notice {
    fn of(reason: &str) -> Self {
        if reason.starts_with(aether_core::agent::COST_FLOOR_NOTE) {
            Self::CostFloor
        } else {
            Self::SoftLimit
        }
    }
}

/// One warning of each kind per agent. Only two kinds exist, so the last few
/// warnings are enough to have seen both.
fn already_warned(runtime: &Runtime, agent: AgentId, notice: Notice) -> Result<bool> {
    Ok(runtime
        .store
        .latest_events(
            8,
            &EventFilter {
                types: vec!["budget.warning".into()],
                ..EventFilter::own_events(agent)
            },
        )?
        .iter()
        .any(|e| match &e.kind {
            EventKind::BudgetWarning { reason, .. } => Notice::of(reason) == notice,
            _ => false,
        }))
}

/// Stop an agent that crossed its grant, from wherever the crossing was seen -
/// the sweep below, or the engine turn that noticed it first. One path, so a
/// budget stop always looks the same: the `budget.exceeded` event, the live
/// subtree cancelled with it, and a terminal status naming the budget. A no-op
/// if something already ended the agent.
pub(crate) fn stop_for_budget(
    runtime: &Arc<Runtime>,
    agent: AgentId,
    usage: aether_core::model::Usage,
    reason: &str,
    found_by: Actor,
) -> Result<()> {
    // Read here, not taken from the caller: `stop_over_budget` needs a row
    // read after the last commit, and a turn holds an in-flight copy.
    let Some(fresh) = still_live(runtime, agent)? else {
        return Ok(());
    };
    stop_over_budget(runtime, &fresh, usage, reason.to_owned(), found_by)
}

/// End the agent as Failed with the reason and take its live subtree with it.
/// `agent` must be a row read after the caller's last commit.
fn stop_over_budget(
    runtime: &Arc<Runtime>,
    agent: &Agent,
    usage: aether_core::model::Usage,
    reason: String,
    found_by: Actor,
) -> Result<()> {
    tracing::warn!(agent = %agent.id, %reason, "budget exhausted; stopping the agent");
    let mut commit = Commit::new().event(
        EventDraft::new(
            // Whoever saw the crossing: this sweep, or the agent's own turn.
            // One stop, but the ledger can still tell them apart.
            found_by,
            EventKind::BudgetExceeded {
                reason: reason.clone(),
                usage,
            },
        )
        .agent(agent.id)
        .run(agent.run),
    );
    stop_and_cancel_subtree(runtime, agent, &mut commit, reason)
}

/// End an agent as Failed for a reason it cannot recover from, and take its
/// live subtree with it. `commit` carries whatever event explains the stop.
fn stop_and_cancel_subtree(
    runtime: &Arc<Runtime>,
    stale: &Agent,
    commit: &mut Commit,
    reason: String,
) -> Result<()> {
    // Stop the turn first so it does not run on past the limit while we commit.
    let running = runtime
        .sched
        .lock()
        .running
        .get_mut(&stale.id)
        .map(|running| {
            running.stop_reason.get_or_insert_with(|| reason.clone());
            running.cancel.clone()
        });
    if let Some(token) = running {
        token.cancel();
    }
    let hold = runtime.hold();
    // Re-read under the hold: the caller's row was read before it, and a stop
    // another path committed in between would otherwise be overwritten here,
    // ending the agent twice.
    let Some(agent) = still_live(runtime, stale.id)? else {
        return Ok(());
    };
    let intent = crate::cancellation::prepare(
        runtime,
        &hold,
        agent.id,
        Some(format!("{} stopped: {reason}", agent.id)),
        Actor::Scheduler,
    )?;
    commit.events.extend(intent.events);
    commit.mutations.extend(intent.mutations);
    let reap = runtime.terminate_agent(
        &hold,
        commit,
        agent.clone(),
        AgentStatus::Failed {
            error: reason.clone(),
        },
        Actor::Scheduler,
    )?;
    runtime.apply_held(hold, std::mem::take(commit))?;
    // Only once the stop is durable: a stopped agent's background processes
    // outlive nothing either.
    runtime.reap_ended_agent(reap);
    crate::cancellation::process(runtime, agent.id)?;
    Ok(())
}
