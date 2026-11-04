//! The Noesis admission guard: the one check between a Noesis agent and new work.
//!
//! A Noesis agent is an ordinary agent whose store row names subsystem
//! [`NOESIS_SUBSYSTEM`], fixed at creation; its role label is not trusted for
//! this. The daemon owns the Noesis service and switches the [`NoesisGate`].
//! While the gate is not Enabled the guard refuses these agents new work and
//! parks them; while it is, every turn is admitted by, and charged to, the
//! Noesis budget.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aether_core::agent::Agent;
use aether_core::event::Actor;
use aether_core::id::AgentId;
use aether_core::idempotency::{Submission, SubmissionOutcome};
use aether_core::model::Usage;
use tokio_util::sync::CancellationToken;

use crate::{Result, Runtime, RuntimeError};

mod admission;
pub(crate) use admission::{AdmittedTurn, Dispatch, run_admitted};

/// The `agents.subsystem` value of every Noesis agent.
pub const NOESIS_SUBSYSTEM: &str = "noesis";

/// Roles reserved for Noesis agents; creating one needs the subsystem too.
pub const NOESIS_ROLE_PREFIX: &str = "noesis.";

/// Roles whose turns are background jobs rather than a person's session.
const BACKGROUND_ROLES: &[&str] = &["noesis.extractor", "noesis.auditor"];

/// How often `cancel_noesis_turns` looks whether the interrupted turns ended.
const CANCEL_POLL: Duration = Duration::from_millis(20);

/// Which Noesis budget and concurrency cap a turn counts against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoesisLane {
    Interactive,
    Background,
}

/// The lane a Noesis agent's turns run in, by its role.
pub fn lane_of(role: &str) -> NoesisLane {
    if BACKGROUND_ROLES.contains(&role) {
        NoesisLane::Background
    } else {
        NoesisLane::Interactive
    }
}

/// The Noesis budgets, as the daemon's Noesis service keeps them.
pub trait NoesisBudget: Send + Sync + std::fmt::Debug {
    /// Admit one turn of a Noesis agent before any model call; Err refuses it with no call made.
    fn admit(
        &self,
        agent: &Agent,
        lane: NoesisLane,
    ) -> std::result::Result<Box<dyn NoesisCharge>, NoesisRefusal>;
}

/// One admitted turn's claim on the budget, settled once the turn ends.
pub trait NoesisCharge: Send {
    fn settle(self: Box<Self>, usage: &Usage);
}

/// Why the budget refused a turn.
#[derive(Debug, Clone, PartialEq)]
pub enum NoesisRefusal {
    /// A concurrency cap is full: the turn is offered again shortly.
    Busy(String),
    /// A budget or the billing behind it is spent: the agent is parked.
    Exhausted(String),
}

#[derive(Debug, Clone, Default)]
enum Mode {
    #[default]
    Disabled,
    Enabled(Arc<dyn NoesisBudget>),
    Draining,
}

/// Whether Noesis agents may take new work. Clones share one switch, so the
/// daemon keeps a clone and the runtime reads the same state.
#[derive(Debug, Clone, Default)]
pub struct NoesisGate {
    mode: Arc<parking_lot::RwLock<Mode>>,
}

impl NoesisGate {
    /// A gate that admits nothing until enabled.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enable(&self, budget: Arc<dyn NoesisBudget>) {
        *self.mode.write() = Mode::Enabled(budget);
    }

    /// Refuse new work; turns already running continue. A disabled gate stays disabled.
    pub fn drain(&self) {
        let mut mode = self.mode.write();
        if matches!(*mode, Mode::Enabled(_)) {
            *mode = Mode::Draining;
        }
    }

    pub fn disable(&self) {
        *self.mode.write() = Mode::Disabled;
    }

    pub fn is_enabled(&self) -> bool {
        matches!(*self.mode.read(), Mode::Enabled(_))
    }

    /// Neither admitting nor draining: no Noesis turn may start at all.
    pub fn is_disabled(&self) -> bool {
        matches!(*self.mode.read(), Mode::Disabled)
    }

    /// The budget that admits turns, only while the gate is Enabled.
    fn budget(&self) -> Option<Arc<dyn NoesisBudget>> {
        match &*self.mode.read() {
            Mode::Enabled(budget) => Some(budget.clone()),
            Mode::Disabled | Mode::Draining => None,
        }
    }
}

/// A Noesis agent as the guard found it.
pub(crate) struct NoesisAdmission {
    pub(crate) lane: NoesisLane,
    /// Present only while the gate is Enabled; `None` refuses new work.
    pub(crate) budget: Option<Arc<dyn NoesisBudget>>,
}

impl Runtime {
    pub fn noesis_gate(&self) -> &NoesisGate {
        &self.config.noesis
    }

    /// The one guard: `None` for an agent outside Noesis, which it leaves
    /// alone. Reads the agent's fixed subsystem once and the gate once.
    pub(crate) fn noesis_admission(&self, agent: &Agent) -> Result<Option<NoesisAdmission>> {
        let subsystem = self.store.agent_subsystem(agent.id)?;
        debug_assert!(
            subsystem
                .as_deref()
                .is_none_or(|name| !name.trim().is_empty()),
            "a subsystem is named or absent"
        );
        if subsystem.as_deref() != Some(NOESIS_SUBSYSTEM) {
            return Ok(None);
        }
        let lane = lane_of(&agent.role.0);
        debug_assert!(
            lane == NoesisLane::Interactive || agent.role.0.starts_with(NOESIS_ROLE_PREFIX),
            "only a noesis role runs in the background lane"
        );
        Ok(Some(NoesisAdmission {
            lane,
            budget: self.config.noesis.budget(),
        }))
    }

    /// Refuse a command that would give `agent` new work while Noesis is off
    /// or draining. Called before the command changes anything.
    pub(crate) fn refuse_noesis_while_off(&self, agent: &Agent) -> Result<()> {
        match self.noesis_admission(agent)? {
            Some(admission) if admission.budget.is_none() => {
                Err(RuntimeError::NoesisDisabled(agent.id))
            }
            _ => Ok(()),
        }
    }

    /// Refuse creating an agent that would belong to Noesis, by subsystem or
    /// by reserved role, unless it is created into Noesis while the gate is Enabled.
    pub(crate) fn check_noesis_creation(&self, role: &str, subsystem: Option<&str>) -> Result<()> {
        let into_noesis = subsystem == Some(NOESIS_SUBSYSTEM);
        if is_noesis_role(role) && !into_noesis {
            return Err(RuntimeError::Invalid(format!(
                "role {role} is reserved for agents of subsystem {NOESIS_SUBSYSTEM}"
            )));
        }
        // Spend is read back by role, so membership and role must agree.
        if into_noesis && !is_noesis_role(role) {
            return Err(RuntimeError::Invalid(format!(
                "an agent of subsystem {NOESIS_SUBSYSTEM} needs a {NOESIS_ROLE_PREFIX}* role, not {role}"
            )));
        }
        if into_noesis && !self.config.noesis.is_enabled() {
            return Err(RuntimeError::Invalid(
                "Noesis is disabled: no agent may be created into it".into(),
            ));
        }
        Ok(())
    }

    /// Refuse a child outside Noesis under a Noesis parent: it would escape the gate.
    pub(crate) fn check_noesis_parent(
        &self,
        parent: &Agent,
        subsystem: Option<&str>,
    ) -> Result<()> {
        if subsystem == Some(NOESIS_SUBSYSTEM) {
            return Ok(());
        }
        if self.store.agent_subsystem(parent.id)?.as_deref() == Some(NOESIS_SUBSYSTEM) {
            return Err(RuntimeError::Invalid(format!(
                "agent {} belongs to Noesis: its children must too",
                parent.id
            )));
        }
        Ok(())
    }

    /// Hold a Noesis agent the guard refused: a pause by the system, durable,
    /// so neither the scheduler nor a restart resumes it by surprise.
    pub(crate) fn park_noesis(&self, id: AgentId, why: &str) -> Result<()> {
        let outcome = self.pause_agent(id, false, Actor::System)?;
        debug_assert!(
            outcome
                .changed
                .iter()
                .all(|agent| agent.id == id && agent.paused),
            "a plain pause holds the one agent"
        );
        if !outcome.changed.is_empty() {
            tracing::info!(agent = %id, reason = why, "parked a Noesis agent");
        }
        Ok(())
    }

    /// Noesis agents with a turn in flight, and the lane each runs in.
    pub fn noesis_turns_running(&self) -> Vec<(AgentId, NoesisLane)> {
        self.sched
            .lock()
            .running
            .iter()
            .filter_map(|(id, turn)| turn.noesis.map(|lane| (*id, lane)))
            .collect()
    }

    /// Interrupt the Noesis turns in flight (of `lane`, or all), keeping the
    /// agents, and wait for them to end. Returns how many were interrupted.
    pub async fn cancel_noesis_turns(&self, lane: Option<NoesisLane>) -> Result<usize> {
        let interrupted: Vec<(AgentId, CancellationToken)> = self
            .sched
            .lock()
            .running
            .iter()
            .filter(|(_, turn)| {
                turn.noesis
                    .is_some_and(|l| lane.is_none_or(|want| want == l))
            })
            .map(|(id, turn)| (*id, turn.interrupt.clone()))
            .collect();
        for (id, token) in &interrupted {
            tracing::info!(agent = %id, "interrupting a Noesis turn");
            token.cancel();
        }
        let ids: Vec<AgentId> = interrupted.into_iter().map(|(id, _)| id).collect();
        self.await_turns_ended(&ids).await?;
        Ok(ids.len())
    }

    /// Wait until none of `ids` has a turn in flight, as long as an engine's
    /// own wind-down may take.
    async fn await_turns_ended(&self, ids: &[AgentId]) -> Result<()> {
        let deadline = Instant::now() + self.config.engine_park_grace + Duration::from_secs(10);
        loop {
            let still: Vec<AgentId> = {
                let sched = self.sched.lock();
                ids.iter()
                    .copied()
                    .filter(|id| sched.running.contains_key(id))
                    .collect()
            };
            if still.is_empty() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(RuntimeError::Internal(format!(
                    "interrupted Noesis turns still running: {}",
                    still
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            tokio::time::sleep(CANCEL_POLL).await;
        }
    }
}

impl Runtime {
    /// The receipt a keyed daemon-side Noesis command recorded, if any; a
    /// replay-only submission with none left is an error, as for any command.
    pub fn noesis_receipt(
        &self,
        submission: Option<&Submission>,
    ) -> Result<Option<serde_json::Value>> {
        match self.recorded_submission(submission)? {
            None => Ok(None),
            Some(SubmissionOutcome::Receipt { value }) => Ok(Some(value)),
            Some(other) => Err(crate::submission::wrong_shape(&other, "receipt")),
        }
    }
}

/// Add `submission`'s receipt to `commit`, so the record lands with the work.
pub fn record_noesis_receipt(
    commit: &mut aether_store::Commit,
    submission: Option<&Submission>,
    receipt: &impl serde::Serialize,
) -> Result<()> {
    crate::submission::record_receipt(commit, submission, receipt)
}

/// Whether a turn in `lane` must end before its next model call: a Noesis
/// background job stops there once the gate leaves Enabled.
pub(crate) fn stops_at_model_boundary(runtime: &Runtime, lane: Option<NoesisLane>) -> bool {
    lane == Some(NoesisLane::Background) && !runtime.config.noesis.is_enabled()
}

/// Whether the role is reserved for Noesis agents.
pub(crate) fn is_noesis_role(role: &str) -> bool {
    role.starts_with(NOESIS_ROLE_PREFIX)
}

#[cfg(test)]
mod tests;
