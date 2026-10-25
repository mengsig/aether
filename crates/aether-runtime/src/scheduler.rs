//! Deterministic scheduling: who wakes, who runs, and when.
//!
//! A tick runs after every commit, on a coarse timer for deadlines, and never
//! polls a model. Waiting agents wake only when their condition holds; ready
//! agents with nothing actionable are parked as waiting; the rest run up to
//! the concurrency limit, skipping providers that asked us to back off. An
//! agent a person has just spoken to is never held back by the limit: the
//! children it is supervising cannot make it deaf.

use crate::{Result, Runtime, RuntimeError, wait::RuntimeOracle};
use aether_core::{
    agent::{Agent, AgentStatus, BlockReason},
    time::now,
};
use aether_store::AgentFilter;
use std::{sync::Arc, time::Duration};

mod budgets;
mod dependencies;
mod dispatch;
mod hard_stop;
mod lineage;
mod providers;
mod wake;

pub(crate) use budgets::stop_for_budget;
#[cfg(test)]
use budgets::warn_once;
use budgets::{enforce_budgets, enforce_deadlines};
use dependencies::gate_dependencies;
use dispatch::dispatch;
pub(crate) use dispatch::{claim_link, hold_after_entry_refusal};
use hard_stop::enforce_hard_stops;
use lineage::Lineage;
pub(crate) use lineage::{CapHolder, ResumeLimits, cap_holder, on_provider_descendants};
pub(crate) use providers::{
    QUOTA_BACKOFF, backoff, clear_bench, clear_probe_claim, provider_recovered_by_probe,
    record_error, record_fallback, record_quota, release_probe_claim_of,
};
use providers::{due_canaries, expire_backoff};
pub(crate) use wake::{has_transcript_work, has_unread_inbox, has_work, urgent_inbox};
use wake::{park, person_message_pending, person_reaches, satisfied_wait_note, wake, wake_reason};

pub(crate) fn spawn(runtime: Arc<Runtime>) {
    let supervising = runtime.clone();
    tokio::spawn(async move {
        crate::supervision::restart(
            "scheduler",
            supervising.shutdown.clone(),
            |detail| supervising.report_problem("background.scheduler".into(), detail),
            || run(runtime.clone()),
        )
        .await;
    });
}

async fn run(runtime: Arc<Runtime>) {
    loop {
        tokio::select! {
            _ = runtime.notify.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            _ = runtime.shutdown.cancelled() => break,
        }
        if let Some(Err(e)) = contained(&runtime, "tick", || tick(&runtime)) {
            tracing::error!(error = %e, "scheduler tick failed");
        }
    }
}

/// A fault the tests inject into a named step.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub(crate) enum StepFault {
    FailOnce,
    FailAlways,
    Panic,
}

/// Run one step so a panic cannot unwind past it: the panic is logged with
/// the step's name and counted. `None` means the step panicked.
fn contained<T>(
    runtime: &Runtime,
    step: &'static str,
    run: impl FnOnce() -> Result<T>,
) -> Option<Result<T>> {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(test)]
        injected_fault(runtime, step)?;
        run()
    }));
    match outcome {
        Ok(result) => Some(result),
        Err(payload) => {
            let detail = crate::turn::panic_message(payload.as_ref());
            let total = {
                let mut sched = runtime.sched.lock();
                sched.step_panics += 1;
                sched.step_panics
            };
            runtime.report_problem(format!("scheduler.{step}"), format!("Scheduler step {step} panicked ({total} panics): {detail}; it will retry. Run `aether doctor` and `aether diagnose`."));
            None
        }
    }
}

#[cfg(test)]
fn injected_fault(runtime: &Runtime, step: &'static str) -> Result<()> {
    let fault = runtime.sched.lock().step_faults.get(step).copied();
    match fault {
        None => Ok(()),
        Some(StepFault::Panic) => panic!("injected panic in {step}"),
        Some(fault) => {
            if matches!(fault, StepFault::FailOnce) {
                runtime.sched.lock().step_faults.remove(step);
            }
            Err(crate::RuntimeError::Internal(format!(
                "injected failure in {step}"
            )))
        }
    }
}

/// What failed during one pass.
#[derive(Default)]
struct Faults {
    errors: Vec<RuntimeError>,
    /// A step that keeps agents within their limits failed or panicked: no
    /// turn starts this pass, because it could run past a cancellation,
    /// deadline, budget or unmet dependency.
    withhold_dispatch: bool,
}

/// Run a step whose failure is logged and kept but never skips the steps
/// after it. Returns whether it completed.
fn isolated(
    runtime: &Runtime,
    faults: &mut Faults,
    step: &'static str,
    run: impl FnOnce() -> Result<()>,
) -> bool {
    match contained(runtime, step, run) {
        Some(Ok(())) => {
            runtime.clear_problem(&format!("scheduler.{step}"));
            true
        }
        Some(Err(e)) => {
            runtime.report_problem(format!("scheduler.{step}"), format!("Scheduler step {step} failed: {e}; it will retry. Run `aether doctor` and `aether diagnose`."));
            faults.errors.push(e);
            false
        }
        None => false,
    }
}

/// Like `isolated`, for steps that enforce limits: a failure also withholds
/// this pass's dispatch.
fn guarded(
    runtime: &Runtime,
    faults: &mut Faults,
    step: &'static str,
    run: impl FnOnce() -> Result<()>,
) {
    if !isolated(runtime, faults, step, run) {
        faults.withhold_dispatch = true;
    }
}

/// One scheduling pass. Public for tests. Every step runs even when an
/// earlier one failed; the first failure is returned once the pass is done.
pub fn tick(runtime: &Arc<Runtime>) -> Result<()> {
    if runtime
        .recovery_failed
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        return Ok(());
    }
    let mut faults = Faults::default();
    pass(runtime, &mut faults)?;
    match faults.errors.into_iter().next() {
        Some(first) => Err(first),
        None => Ok(()),
    }
}

fn pass(runtime: &Arc<Runtime>, faults: &mut Faults) -> Result<()> {
    let cancelling = crate::cancellation::pending_agents(runtime)?;
    isolated(runtime, faults, "drain cancellations", || {
        crate::cancellation::drain(runtime)
    });
    isolated(runtime, faults, "expire backoff", || {
        expire_backoff(runtime)
    });
    isolated(runtime, faults, "sweep orphans", || {
        crate::orphans::sweep_if_due(runtime)
    });
    let mut blocked = std::collections::HashSet::new();
    let mut record_errors = Vec::new();
    guarded(runtime, faults, "enforce budgets", || {
        enforce_budgets(runtime, &mut blocked, &mut record_errors)
    });
    let live = live_after_enforcement(runtime, faults, &mut blocked, &mut record_errors)?;
    let lineage = Lineage::from_store(&live, runtime);
    let mut ready: Vec<Runnable> = Vec::new();
    let at = now();
    let canaries = due_canaries(runtime, &live, at);
    for agent in live {
        // VISION §3.8: a paused agent receives no turns; its inbox fills.
        if agent.paused
            || cancelling.contains(&agent.id)
            || blocked.contains(&agent.id)
            || lineage.has_unreadable_ancestor(agent.id)
        {
            continue;
        }
        let id = agent.id;
        let result = record_work(|| prepare_agent(runtime, agent, &canaries));
        match result {
            Ok(candidate) => {
                runtime.clear_problem(&format!("scheduler.agent.{id}"));
                ready.extend(candidate);
            }
            Err(error) => runtime.report_problem(format!("scheduler.agent.{id}"), format!(
                "Agent {id} could not be scheduled: {error}; it is withheld and will retry. Run `aether doctor` and `aether diagnose`.")),
        }
    }
    faults.errors.extend(record_errors);
    // Before the dispatch gates below: a held run's turns stop even when no
    // new turn may start.
    let mut reserve = enforce_hard_stops(runtime)?;
    // Even a person's message waits: every turn writes, and a disk this low
    // is about to refuse writes.
    if !runtime.disk_is_low() && !faults.withhold_dispatch {
        dispatch(runtime, ready, &lineage, &mut reserve);
    }
    reserve.finish(runtime);
    Ok(())
}

fn record_work<T>(work: impl FnOnce() -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_else(|panic| {
        Err(RuntimeError::Invalid(format!(
            "record processing panicked: {}; run `aether doctor`",
            crate::turn::panic_message(panic.as_ref())
        )))
    })
}

fn prepare_agent(
    runtime: &Arc<Runtime>,
    agent: Agent,
    canaries: &std::collections::HashMap<String, aether_core::id::AgentId>,
) -> Result<Option<Runnable>> {
    let woken = match &agent.status {
        // Limit 1 outranks the gate: a person's words reach an agent the
        // scheduler is holding back, and the gate re-applies after the
        // reply turn.
        AgentStatus::Blocked { reason } if person_reaches(runtime, &agent, reason)? => wake(
            runtime,
            agent,
            "message from the user".to_owned(),
            None,
            None,
        )?,
        // The probe is the interrupted turn itself: Ready is all it needs,
        // because the user message it never answered is already committed.
        AgentStatus::Blocked {
            reason: BlockReason::Exhausted { model, .. },
        } if canaries.get(&model.provider) == Some(&agent.id) => {
            let (provider, id) = (model.provider.clone(), agent.id);
            if !providers::claim_canary(runtime, &provider, id) {
                return Ok(None);
            }
            let woken = wake(
                runtime,
                agent,
                format!("probing {provider} after exhaustion"),
                None,
                None,
            );
            if !matches!(&woken, Ok(Some(_))) {
                runtime.sched.lock().probing.remove(&provider);
            }
            woken?
        }
        AgentStatus::Waiting { .. } => wake_waiting(runtime, agent)?,
        AgentStatus::Ready => {
            if has_work(runtime, &agent)? {
                Some(agent)
            } else {
                park(runtime, agent)?;
                None
            }
        }
        _ => None,
    };
    if let Some(agent) = woken {
        let addressed = person_message_pending(runtime, &agent)?;
        return Ok(Some(Runnable { agent, addressed }));
    }
    Ok(None)
}

fn wake_waiting(runtime: &Runtime, agent: Agent) -> Result<Option<Agent>> {
    let AgentStatus::Waiting { condition } = &agent.status else {
        return Ok(None);
    };
    Ok({
        let oracle = RuntimeOracle {
            runtime,
            agent: agent.id,
            inbox_cursor: agent.inbox_cursor,
        };
        if condition.is_satisfied(&oracle) {
            let reason = wake_reason(condition, &oracle);
            let note = satisfied_wait_note(runtime, &agent, condition, &oracle)?;
            wake(runtime, agent, reason, None, note)?
        } else if !condition.involves_approval()
            && let Some(reason) = urgent_inbox(runtime, &agent)?
        {
            // Sleeping never means deaf: a person, this agent's own
            // parent, or a failure gets through, and the wait is
            // restored after the turn.
            let interrupted = condition.clone();
            wake(runtime, agent, reason, Some(interrupted), None)?
        } else {
            None
        }
    })
}

/// Reuse the deadline snapshot unless enforcement or dependency gates changed it.
fn live_after_enforcement(
    runtime: &Arc<Runtime>,
    faults: &mut Faults,
    blocked: &mut std::collections::HashSet<aether_core::id::AgentId>,
    record_errors: &mut Vec<RuntimeError>,
) -> Result<Vec<Agent>> {
    let watermark = runtime.store.commits();
    let filter = AgentFilter {
        live_only: true,
        ..Default::default()
    };
    let mut live = runtime.store.agents(&filter)?;
    guarded(runtime, faults, "enforce deadlines", || {
        enforce_deadlines(runtime, &live, blocked, record_errors)
    });
    isolated(runtime, faults, "fire schedules", || {
        crate::schedules::tick(runtime, now())
    });
    guarded(runtime, faults, "gate dependencies", || {
        gate_dependencies(runtime, blocked, record_errors)
    });
    if runtime.store.commits() != watermark {
        live = runtime.store.agents(&filter)?;
    }
    debug_assert!(live.iter().all(Agent::is_live));
    debug_assert!(runtime.store.commits() >= watermark);
    Ok(live)
}

/// An agent with something to do this tick.
struct Runnable {
    agent: Agent,
    /// A person is waiting on it: it runs even past the concurrency limit.
    addressed: bool,
}

#[cfg(test)]
mod tests;
