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
use aether_core::model::{ModelRef, Usage};
use tokio_util::sync::CancellationToken;

use crate::{Result, Runtime, RuntimeError};

mod admission;
pub use admission::usage_spent;
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

/// One admitted turn's claim on the budget, shared by the turn that spends
/// it and the task that settles it once the turn ends.
pub trait NoesisCharge: Send + Sync {
    /// Whether the service that admitted the turn still runs; a claim on a
    /// retired one must start nothing.
    fn is_current(&self) -> bool;
    /// The one hook before every model request of the turn (an API invoke,
    /// each compaction attempt, each native usage report or bridge request).
    /// It reports the turn's spend so far so concurrent admissions see it,
    /// and returns `Some(reason)` (Stop) once the budget is spent or the
    /// service that admitted the turn is closed or retired.
    ///
    /// A native engine (Claude Code, Codex) may finish at most ONE model
    /// request already in flight when the budget runs out or a disable
    /// starts: the next usage report or bridge request reaches this hook,
    /// which interrupts the engine.
    fn before_invocation(&self, spent: &Usage) -> Option<String>;
    /// Why `model`, or the compactor it would use, may not carry the turn.
    fn refuses_model(&self, model: &ModelRef) -> Option<String>;
    /// Charge what the turn spent and free its slot; later calls do nothing.
    fn settle(&self, usage: &Usage);
}

/// The Noesis side of a running turn: its lane and the charge it spends.
#[derive(Clone)]
pub(crate) struct NoesisTurn {
    pub(crate) lane: NoesisLane,
    /// The service generation the turn was admitted under.
    pub(crate) generation: u64,
    pub(crate) charge: Arc<dyn NoesisCharge>,
}

impl std::fmt::Debug for NoesisTurn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoesisTurn")
            .field("lane", &self.lane)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
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

#[derive(Debug, Clone, Default)]
struct GateState {
    mode: Mode,
    /// Counts the service's lifetimes: every `enable` and `disable` starts a
    /// new one, and an admission is only good under the one it was granted in.
    generation: u64,
}

/// Whether Noesis agents may take new work. Clones share one switch, so the
/// daemon keeps a clone and the runtime reads the same state.
#[derive(Debug, Clone, Default)]
pub struct NoesisGate {
    state: Arc<parking_lot::RwLock<GateState>>,
}

impl NoesisGate {
    /// A gate that admits nothing until enabled.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enable(&self, budget: Arc<dyn NoesisBudget>) {
        let mut state = self.state.write();
        state.mode = Mode::Enabled(budget);
        state.generation += 1;
    }

    /// Refuse new work; turns already running continue. A disabled gate stays disabled.
    pub fn drain(&self) {
        let mut state = self.state.write();
        if matches!(state.mode, Mode::Enabled(_)) {
            state.mode = Mode::Draining;
        }
    }

    pub fn disable(&self) {
        let mut state = self.state.write();
        state.mode = Mode::Disabled;
        state.generation += 1;
    }

    pub fn is_enabled(&self) -> bool {
        matches!(self.state.read().mode, Mode::Enabled(_))
    }

    /// Neither admitting nor draining: no Noesis turn may start at all.
    pub fn is_disabled(&self) -> bool {
        matches!(self.state.read().mode, Mode::Disabled)
    }

    /// The current service generation.
    pub fn generation(&self) -> u64 {
        self.state.read().generation
    }

    /// Whether an admission granted under `generation` may still start work:
    /// the gate has not been disabled, and no disable or restart has begun
    /// a newer generation. A draining gate lets what it already admitted start.
    pub fn admits(&self, generation: u64) -> bool {
        let state = self.state.read();
        !matches!(state.mode, Mode::Disabled) && state.generation == generation
    }

    /// The budget that admits turns and the generation it admits under, read
    /// together, only while the gate is Enabled.
    fn budget(&self) -> Option<(Arc<dyn NoesisBudget>, u64)> {
        let state = self.state.read();
        match &state.mode {
            Mode::Enabled(budget) => Some((budget.clone(), state.generation)),
            Mode::Disabled | Mode::Draining => None,
        }
    }
}

/// A Noesis agent as the guard found it.
pub(crate) struct NoesisAdmission {
    pub(crate) lane: NoesisLane,
    /// Present only while the gate is Enabled, with the generation it was
    /// read under; `None` refuses new work.
    pub(crate) budget: Option<(Arc<dyn NoesisBudget>, u64)>,
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
            .filter_map(|(id, turn)| turn.noesis.as_ref().map(|noesis| (*id, noesis.lane)))
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
                    .as_ref()
                    .is_some_and(|noesis| lane.is_none_or(|want| want == noesis.lane))
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

    /// Record one model call a Noesis agent made outside the scheduler: its
    /// usage and one more turn, with what `with` adds from the fresh row, in
    /// one commit. The row is reread under the commit hold and only its
    /// `stats` change, so a pause, cancel, configuration or approval change
    /// committed while the call ran stands. Returns the row as written.
    pub fn record_noesis_call(
        &self,
        id: AgentId,
        usage: Usage,
        with: impl FnOnce(&Agent, &mut aether_store::Commit),
    ) -> Result<Agent> {
        debug_assert!(usage.cost_usd >= 0.0, "a call never refunds: {usage:?}");
        let hold = self.hold();
        let mut agent = self
            .store
            .agent(id)?
            .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?;
        if !is_noesis_role(agent.role.as_str()) {
            return Err(RuntimeError::Invalid(format!(
                "agent {id} is not a Noesis agent"
            )));
        }
        let turns = agent.stats.turns;
        agent.stats.usage += usage;
        agent.stats.turns = turns.saturating_add(1);
        debug_assert!(agent.stats.turns > turns || turns == u32::MAX);
        let mut commit = aether_store::Commit::new();
        with(&agent, &mut commit);
        commit.push_event(
            aether_core::event::EventDraft::new(
                Actor::agent(agent.id),
                aether_core::event::EventKind::AgentUsageUpdated {
                    total: agent.stats.usage,
                    turn_usage: usage,
                },
            )
            .agent(agent.id)
            .run(agent.run),
        );
        commit.push_mutation(aether_store::Mutation::UpsertAgent(agent.clone()));
        self.apply_held(hold, commit)?;
        Ok(agent)
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

/// Why a Noesis turn must not make another model call: a background job
/// once the gate left Enabled, or any turn whose spend used up its budget.
pub(crate) fn boundary_stop(runtime: &Runtime, ctx: &crate::turn::TurnCtx) -> Option<String> {
    let turn = ctx.noesis.as_ref()?;
    debug_assert!(
        ctx.turn_usage.cost_usd.is_finite(),
        "a turn's spend is a real amount"
    );
    if stops_at_model_boundary(runtime, Some(turn.lane)) {
        return Some("Noesis is not enabled".to_owned());
    }
    turn.charge.before_invocation(&ctx.turn_usage)
}

/// Why a Noesis turn may not start its engine now: it is not admitted under
/// the current generation of an Enabled gate, or the hook says stop. Asked
/// right before the engine starts, after every await that preceded it.
pub(crate) fn engine_start_refusal(
    runtime: &Runtime,
    ctx: &crate::turn::TurnCtx,
) -> Option<String> {
    let turn = ctx.noesis.as_ref()?;
    if !runtime.config.noesis.admits(turn.generation) {
        return Some("Noesis was disabled or restarted since this turn was admitted".to_owned());
    }
    boundary_stop(runtime, ctx)
}

/// At a native engine's model boundary: interrupt the turn when
/// [`boundary_stop`] says so. The engine winds down as for any interrupt.
pub(crate) fn interrupt_at_boundary(runtime: &Runtime, ctx: &crate::turn::TurnCtx) {
    let Some(reason) = boundary_stop(runtime, ctx) else {
        return;
    };
    let sched = runtime.sched.lock();
    let Some(running) = sched.running.get(&ctx.agent.id) else {
        debug_assert!(false, "a turn at its model boundary holds its slot");
        return;
    };
    if !running.interrupt.is_cancelled() {
        tracing::info!(agent = %ctx.agent.id, %reason, "interrupting a Noesis turn at its model boundary");
        running.interrupt.cancel();
    }
}

/// Why the Noesis billing rule forbids `model` for this turn, if it does.
pub(crate) fn refused_model(ctx: &crate::turn::TurnCtx, model: &ModelRef) -> Option<String> {
    let turn = ctx.noesis.as_ref()?;
    turn.charge.refuses_model(model)
}

/// Whether the role is reserved for Noesis agents.
pub(crate) fn is_noesis_role(role: &str) -> bool {
    role.starts_with(NOESIS_ROLE_PREFIX)
}

#[cfg(test)]
mod tests;
