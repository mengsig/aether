//! Running agents that have no turn. A turn that could not record its own
//! end leaves its agent marked running with nothing driving it and nothing
//! scheduled to wake it; the sweep finds those and recovers them as a
//! restart would.

use std::sync::Arc;
use std::time::Duration;

use aether_core::agent::{Agent, AgentStatusKind};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::AgentId;
use aether_core::lease::Lease;
use aether_core::time::{Timestamp, now};
use aether_store::{AgentFilter, Commit};

use crate::{Result, Runtime};

/// The event a recovered orphan leaves on the agent.
const AGENT_RECOVERED: &str = "agent.recovered";

/// Why the sweep recovered the agent, as the event names it.
const REASON_NO_TURN: &str = "running with no turn in flight";

/// Whether an agent is marked running with no turn and has been silent for
/// at least `idle` at `at`. Silence runs from its last event or, failing
/// that, its last update.
pub(crate) fn is_orphan(
    agent: &Agent,
    turn_in_flight: bool,
    last_event_at: Option<Timestamp>,
    at: Timestamp,
    idle: Duration,
) -> bool {
    if agent.status.kind() != AgentStatusKind::Running || turn_in_flight {
        return false;
    }
    let quiet_since = last_event_at.map_or(agent.updated_at, |e| e.max(agent.updated_at));
    at.signed_duration_since(quiet_since)
        .to_std()
        .is_ok_and(|quiet| quiet >= idle)
}

/// Whether a sweep is due: none has run, or `interval` has passed since.
/// A zero interval means the sweep is off.
pub(crate) fn sweep_due(last: Option<Timestamp>, at: Timestamp, interval: Duration) -> bool {
    if interval.is_zero() {
        return false;
    }
    last.is_none_or(|last| {
        at.signed_duration_since(last)
            .to_std()
            .is_ok_and(|since| since >= interval)
    })
}

impl Runtime {
    /// Whether a turn of this agent is running in this daemon now.
    pub fn turn_in_flight(&self, agent: AgentId) -> bool {
        let sched = self.sched.lock();
        sched.running.contains_key(&agent) || sched.engine_retakers.contains(&agent)
    }

    /// Fill in the reply-only liveness fields of agents about to be sent out.
    pub fn annotate_agents(&self, agents: &mut [Agent]) -> Result<()> {
        for agent in agents.iter_mut() {
            agent.engine_alive = self.turn_in_flight(agent.id);
            agent.last_event_at = self.store.last_event_at(agent.id)?;
        }
        Ok(())
    }
}

/// The scheduler's step: sweep when the configured interval has passed.
pub(crate) fn sweep_if_due(runtime: &Arc<Runtime>) -> Result<()> {
    let at = now();
    let config = &runtime.config;
    {
        let mut sched = runtime.sched.lock();
        if !sweep_due(sched.last_orphan_sweep, at, config.orphan_sweep_interval) {
            return Ok(());
        }
        sched.last_orphan_sweep = Some(at);
    }
    let recovered = sweep(runtime, at, config.orphan_idle)?;
    if !recovered.is_empty() {
        tracing::warn!(
            agents = recovered.len(),
            "recovered running agents with no turn"
        );
    }
    Ok(())
}

/// Recover every orphan at `at`, returning the agents it recovered. Each
/// leaves an `agent.recovered` event naming why.
pub(crate) fn sweep(runtime: &Arc<Runtime>, at: Timestamp, idle: Duration) -> Result<Vec<AgentId>> {
    let running = runtime.store.agents(&AgentFilter {
        statuses: vec![AgentStatusKind::Running],
        ..Default::default()
    })?;
    let mut recovered = Vec::new();
    for agent in running {
        let last_event = runtime.store.last_event_at(agent.id)?;
        if !is_orphan(
            &agent,
            runtime.turn_in_flight(agent.id),
            last_event,
            at,
            idle,
        ) {
            continue;
        }
        recover(runtime, &agent)?;
        recovered.push(agent.id);
    }
    debug_assert!(recovered.iter().all(|id| !runtime.turn_in_flight(*id)));
    Ok(recovered)
}

fn recover(runtime: &Arc<Runtime>, agent: &Agent) -> Result<()> {
    tracing::warn!(agent = %agent.id, "running agent has no turn; recovering it");
    // Same stand-in a restart uses for an agent running without a lease.
    let lease = match runtime.store.lease(agent.id)? {
        Some(lease) => lease,
        None => Lease::new(
            agent.id,
            "unknown",
            agent.stats.turns,
            now(),
            Duration::ZERO,
        ),
    };
    crate::recovery::recover_leases(runtime, vec![lease], false)?;
    runtime.apply(Commit::new().event(recovered_event(agent)))?;
    Ok(())
}

fn recovered_event(agent: &Agent) -> EventDraft {
    EventDraft::new(
        Actor::System,
        EventKind::Custom {
            name: AGENT_RECOVERED.into(),
            payload: serde_json::json!({ "reason": REASON_NO_TURN }),
        },
    )
    .agent(agent.id)
    .parent(agent.parent)
    .run(agent.run)
}

#[cfg(test)]
mod tests;
