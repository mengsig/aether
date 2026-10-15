//! One agent turn: settle pending approvals, assemble context, invoke the
//! model, run tool calls, apply harness actions, and decide the next state.
//! Every step is an atomic commit, so a crash anywhere leaves a state the
//! recovery pass can classify.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use aether_context::ContextInputs;
use aether_core::agent::{
    Agent, AgentStatus, AgentStatusKind, BlockReason, ExhaustionClass, WaitCondition,
};
use aether_core::event::{Actor, CompactionTrigger, EventDraft, EventKind};
use aether_core::id::{AgentId, ApprovalId, ArtifactId, InvocationId, ToolCallId};
use aether_core::idempotency::RecordedOutcome;
use aether_core::inbox::InboxKind;
use aether_core::lease::Lease;
use aether_core::memory::MemoryScope;
use aether_core::model::{
    ContentBlock, Message, MessageRole, ModelError, ModelErrorClass, ModelRef, ModelResponse,
    StopReason, StreamEvent, ToolResultBlock,
};
use aether_core::snapshot::Invocation;
use aether_core::task::{Task, TaskResult, TaskStatus, TaskStatusKind};
use aether_core::time::{Timestamp, now};
use aether_core::tool::{
    ApprovalDecision, ApprovalRequest, ApprovalStatus, ResourceKey, ToolCall, ToolOutcome,
};
use aether_memory::RetrievalRequest;
use aether_model::collect_partial;
use aether_store::{
    AgentFilter, Commit, EventFilter, Mutation, StoreError, TaskFilter, TranscriptEntry,
};
use aether_tools::{HarnessAction, Tool, ToolContext, ToolError, ToolExecution, requires_approval};
use tokio_util::sync::CancellationToken;

use crate::approval::{self, Gate};
use crate::artifacts::StoreArtifactSink;
use crate::bus::DeltaKind;
use crate::commands::CreateAgentSpec;
use crate::resources::{ResourceGuard, WaitOutcome};
use crate::{CommitHold, Result, Runtime, RuntimeError};

mod completion;
use completion::mark_completed;
pub(crate) use completion::{CompletionSource, prose_only_next};

mod state;
use state::*;
pub(crate) use state::{agent_row, agent_row_with_session, close_exhaustion_episode};
mod execution;
use execution::*;
mod compaction;
pub(crate) use compaction::{compact, compaction_pending, compaction_pending_for_turn};
mod failure;
use failure::*;
mod context;
pub(crate) use context::{agent_temp_dir, retrieve_memories, tool_context};
mod tools;
pub(crate) use tools::ToolEffects;
use tools::*;
mod resources;
use resources::*;
mod capacity;
use capacity::*;
mod actions;
pub(crate) use actions::apply_action;
pub(crate) use waiting::validate_wait;
mod waiting;
pub(crate) use waiting::has_direct_user_message;
use waiting::*;
pub(crate) use waiting::{NO_ACTIVE_TASK_TO_COMPLETE, NO_ACTIVE_TASK_TO_FAIL, TURN_ENDS_HERE};
mod settlement;
use settlement::*;
mod outage;
use outage::{Ending, StoreOutage};
mod heartbeat;
use heartbeat::spawn_heartbeat;

/// The turn's entry in `sched.running`, released however `run` leaves -
/// including an unwind. The entry *is* the concurrency slot (global, provider
/// and engine), so one leaked entry holds a slot for the daemon's life; under
/// a small engine cap that wedges every engine agent at once.
struct RunningSlot {
    runtime: Arc<Runtime>,
    agent: AgentId,
}

impl Drop for RunningSlot {
    fn drop(&mut self) {
        let engine = {
            let mut sched = self.runtime.sched.lock();
            // However this turn ended - recovered, probe failed, interrupted,
            // terminal, unwound - it is no longer probing anything.
            crate::scheduler::release_probe_claim_of(&mut sched, self.agent);
            sched
                .running
                .remove(&self.agent)
                .is_some_and(|r| r.on_provider && r.engine)
        };
        // Engine waiters first: a resumer queued here is holding resource
        // keys, so it gets first refusal on the slot it just freed.
        if engine {
            self.runtime.engine_slot.notify_waiters();
        }
        self.runtime.notify.notify_one();
    }
}

/// Entry point spawned by the scheduler. Never panics the scheduler: any
/// internal failure, an unwind included, is recorded on the agent.
pub(crate) async fn run(
    runtime: Arc<Runtime>,
    agent_id: AgentId,
    cancel: CancellationToken,
    interrupt: CancellationToken,
) {
    let _slot = RunningSlot {
        runtime: runtime.clone(),
        agent: agent_id,
    };
    // A panic below is a bug, not an expected path, but it must stay visible
    // and must not leave the agent Running forever with a live lease.
    let outcome = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(execute(
        &runtime, agent_id, cancel, interrupt,
    )))
    .await;
    let (ending, outage) = match outcome {
        Ok(Ok(())) => return,
        // The turn's state is whatever its last landed write left, as after
        // a crash; failing it would need a write the disk refuses too.
        Ok(Err(e)) if e.is_store_unwritable() => {
            tracing::error!(
                agent = %agent_id,
                error = %e,
                "the disk refused the turn's write; recovering the turn once it takes writes"
            );
            (Ending::Recover, Some(StoreOutage::began(&e, "the turn")))
        }
        Ok(Err(e)) => {
            tracing::error!(agent = %agent_id, error = %e, "turn failed internally");
            (Ending::Fail(format!("internal error: {e}")), None)
        }
        Err(panic) => {
            let detail = panic_message(panic.as_ref());
            tracing::error!(agent = %agent_id, panic = %detail, "turn panicked");
            let error = format!("internal error: the turn panicked: {detail}");
            (Ending::Fail(error), None)
        }
    };
    outage::settle(&runtime, agent_id, ending, outage).await;
}

/// What a caught unwind carried, for the failure the agent is given.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no message".to_owned())
}

fn fail_agent(runtime: &Runtime, agent_id: AgentId, error: String) -> Result<()> {
    let Some(agent) = runtime.store.agent(agent_id)? else {
        return Ok(());
    };
    if agent.status.is_terminal() {
        return Ok(());
    }
    let hold = runtime.hold();
    let mut commit = Commit::new();
    let reap = runtime.terminate_agent(
        &hold,
        &mut commit,
        agent,
        AgentStatus::Failed { error },
        Actor::System,
    )?;
    runtime.apply_held(hold, commit)?;
    runtime.reap_ended_agent(reap);
    Ok(())
}

pub(crate) struct TurnCtx {
    pub(crate) agent: Agent,
    pub(crate) task: Option<Task>,
    pub(crate) turn: u32,
    pub(crate) turn_usage: aether_core::model::Usage,
    pub(crate) base_actor: Actor,
    /// The whole-agent hard-stop token (parent of the `interrupt` token every
    /// other function in this module threads through as `cancel`). A
    /// non-interruptible harness action ignores interrupt but must still
    /// observe this: see `run_tool_calls`.
    pub(crate) hard_cancel: CancellationToken,
    /// The link whose exhaustion episode this turn has already looked at.
    /// Only the first completed invocation on a link looks: §5's recovered
    /// fence in memory, so an ordinary turn pays no store read per invocation.
    pub(crate) recovery_checked_for: Option<aether_core::model::ModelRef>,
    /// A direct user message received this turn still needs visible assistant text.
    pub(crate) user_reply_pending: bool,
    /// The handoff this model owed when the turn started, before later
    /// selections could replace the ledger's latest request.
    pub(crate) start_handoff: Option<compaction::PendingCompaction>,
}

impl TurnCtx {
    pub(crate) fn add_usage(&mut self, usage: aether_core::model::Usage) {
        self.agent.stats.usage += usage;
        self.turn_usage += usage;
    }

    pub(crate) fn draft(&self, kind: EventKind) -> EventDraft {
        let mut d = EventDraft::new(self.base_actor.clone(), kind)
            .agent(self.agent.id)
            .parent(self.agent.parent)
            .run(self.agent.run);
        d.task_id = self.task.as_ref().map(|t| t.id);
        d
    }
}

/// `interrupt` is a child of `cancel`: the turn observes it and stops on
/// either; which one fired decides whether the agent survives the turn.
async fn execute(
    runtime: &Arc<Runtime>,
    agent_id: AgentId,
    cancel: CancellationToken,
    interrupt: CancellationToken,
) -> Result<()> {
    // ---- 1. begin the turn: Running + lease ---------------------------------
    let Some((mut ctx, lease)) = begin_turn(runtime, agent_id, &cancel)? else {
        return Ok(());
    };
    if let Some(running) = runtime.sched.lock().running.get_mut(&agent_id) {
        running.task = ctx
            .task
            .as_ref()
            .filter(|task| !task.status.is_terminal())
            .map(|task| task.id);
    }

    // Keep the lease alive while the turn runs.
    let heartbeat = spawn_heartbeat(runtime.clone(), lease, &ctx.agent);
    let task_was_open = ctx.task.as_ref().is_some_and(|t| !t.status.is_terminal());
    let result = execute_inner(runtime, &mut ctx, interrupt).await;
    if result.is_ok()
        && !cancel.is_cancelled()
        && task_was_open
        && let Err(e) = request_task_boundary_compaction(runtime, &ctx)
    {
        // Best-effort by design (§6.3): the task result stands either way, but
        // the miss is an event, not just a log line.
        tracing::error!(agent = %agent_id, error = %e, "task-boundary compaction failed");
        runtime.apply(Commit::new().event(ctx.draft(EventKind::CompactionFailed {
            error: format!("task-boundary compaction failed: {e}"),
            models: Vec::new(),
        })))?;
    }
    let refused_renewals = heartbeat.stop();

    // ---- end: release the lease and record the transition -------------------
    let next = match result {
        Ok(Next::Cancelled(_)) if !cancel.is_cancelled() => Next::Interrupted,
        Ok(next) => next,
        // `run` fails the agent, or first waits out a disk refusing writes.
        Err(e) => return Err(e),
    };
    if let Some(refused) = refused_renewals {
        let agent = &ctx.agent;
        runtime.apply(Commit::new().event(refused.draft(agent.id, agent.parent, agent.run)))?;
    }
    finish(runtime, &ctx, next)
}

/// Start the turn: Running, the lease, and an assigned task Active. `None` when
/// the agent is not to run one after all. Sync, so its hold never spans an await.
fn begin_turn(
    runtime: &Runtime,
    agent_id: AgentId,
    cancel: &CancellationToken,
) -> Result<Option<(TurnCtx, Lease)>> {
    // Read and written under one hold: the row is written back whole, so a
    // pause or an update committed in between would be reverted.
    let hold = runtime.hold();
    let mut agent = runtime
        .store
        .agent(agent_id)?
        .ok_or_else(|| RuntimeError::NotFound(agent_id.to_string()))?;
    if agent.status.kind() != AgentStatusKind::Ready {
        return Ok(None);
    }
    // §3.8: the scheduler read `paused` at the top of its tick and did store
    // I/O before dispatching, so a pause taken in that window would otherwise
    // let a whole turn run after the caller was told the pause was in force.
    if agent.paused {
        return Ok(None);
    }
    let turn = agent.stats.turns + 1;
    if let Some(reason) = turn_refused(runtime, &agent, turn)? {
        // `fail_agent` takes the hold itself.
        drop(hold);
        fail_agent(runtime, agent_id, reason)?;
        return Ok(None);
    }
    let at = now();
    agent.transition(AgentStatus::Running, at)?;
    let lease = Lease::new(
        agent_id,
        runtime.config.holder.clone(),
        turn,
        at,
        runtime.config.lease_ttl,
    );
    let mut task = turn_task(runtime, &agent)?;
    let start_handoff = compaction::handoff_at_turn_start(runtime, &agent)?;
    let mut ctx = TurnCtx {
        agent: agent.clone(),
        task: task.clone(),
        turn,
        turn_usage: Default::default(),
        base_actor: Actor::agent(agent_id),
        hard_cancel: cancel.clone(),
        recovery_checked_for: None,
        user_reply_pending: false,
        start_handoff,
    };
    let mut commit = start_commit(&ctx, &lease, task.as_mut(), at)?;
    commit.push_mutation(Mutation::UpsertAgent(agent.clone()));
    runtime.apply_held(hold, commit)?;
    ctx.agent = agent;
    ctx.task = task;
    Ok(Some((ctx, lease)))
}

/// The task the turn carries. §3.2: the owner is not scheduled for a gated
/// task, so a turn it takes for another reason must not carry it either.
fn turn_task(runtime: &Runtime, agent: &Agent) -> Result<Option<Task>> {
    let Some(id) = agent.current_task else {
        return Ok(None);
    };
    let task = runtime.store.task(id)?;
    Ok(task.filter(|t| t.status.kind() != TaskStatusKind::Queued))
}

/// The turn's opening commit: started, its lease, and an assigned task Active.
fn start_commit(
    ctx: &TurnCtx,
    lease: &Lease,
    task: Option<&mut Task>,
    at: Timestamp,
) -> Result<Commit> {
    let mut commit = Commit::new()
        .event(ctx.draft(EventKind::AgentStarted { turn: ctx.turn }))
        .event(ctx.draft(EventKind::LeaseGranted {
            lease: lease.clone(),
        }))
        .mutate(Mutation::SetLease(lease.clone()));
    if let Some(t) = task
        && t.status.kind() == TaskStatusKind::Assigned
    {
        t.transition(TaskStatus::Active, at)?;
        commit.push_event(ctx.draft(EventKind::TaskStarted));
        commit.push_mutation(Mutation::UpsertTask(t.clone()));
    }
    Ok(commit)
}

/// Why the agent may not take turn `turn`: its turn limit, or a budget.
fn turn_refused(runtime: &Runtime, agent: &Agent, turn: u32) -> Result<Option<String>> {
    if let Some(max) = agent.config.max_turns
        && turn > max
    {
        return Ok(Some(format!("turn limit of {max} reached")));
    }
    // Same predicate the scheduler enforces, in case the last turn crossed
    // the line after the tick that dispatched this one.
    if agent.config.token_budget.is_none() && agent.config.cost_budget.is_none() {
        return Ok(None);
    }
    let all = runtime.store.agents(&AgentFilter::default())?;
    let usage = aether_core::budget::subtree_usage(&all)
        .get(&agent.id)
        .copied()
        .unwrap_or_default();
    Ok(agent.budget_exceeded(&usage))
}

/// Preserve the task boundary, but pay for its checkpoint only when more work arrives.
fn request_task_boundary_compaction(runtime: &Runtime, ctx: &TurnCtx) -> Result<()> {
    if runtime
        .models
        .resolve(&ctx.agent.model)?
        .provider
        .protocol
        .is_engine()
    {
        return Ok(());
    }
    let Some(task) = ctx.task.as_ref().filter(|t| t.status.is_terminal()) else {
        return Ok(());
    };
    // A model update may request a handoff while this turn finishes. Keep
    // the pending check and the TaskBoundary request in one commit window so
    // the latter cannot become the latest event and hide that handoff.
    let hold = runtime.hold();
    if !ctx.agent.config.long_lived || compaction_pending(runtime, ctx.agent.id)?.is_some() {
        return Ok(());
    }
    const MIN_ENTRIES: usize = 6;
    let previous = runtime.store.latest_checkpoint(ctx.agent.id)?;
    let after = previous.as_ref().map(|c| c.provenance.to_seq).unwrap_or(0);
    if runtime.store.transcript(ctx.agent.id, after)?.len() < MIN_ENTRIES {
        return Ok(());
    }
    runtime.apply_held(
        hold,
        Commit::new().event(ctx.draft(EventKind::CompactionRequested {
            trigger: CompactionTrigger::TaskBoundary { task: task.id },
        })),
    )?;
    Ok(())
}

/// What the agent does after this turn.
#[derive(Debug, Clone)]
pub(crate) enum Next {
    Ready,
    Waiting(WaitCondition),
    Completed(Option<String>),
    Failed(String),
    Cancelled(Option<String>),
    /// The operator stopped the turn; the agent idles until spoken to.
    Interrupted,
    /// Every link of the chain is spent: park the agent on a probe schedule
    /// instead of failing it, with the task and the descendants left alone.
    Suspended(Box<crate::exhaustion::Exhausted>),
}

#[cfg(test)]
mod tests;
