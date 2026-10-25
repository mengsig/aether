use super::*;

/// What a batch got from the lease table.
pub(super) enum Held {
    /// Keys taken; the guard frees them.
    Keys(ResourceGuard),
    /// The batch mutates nothing that needs a lease.
    NoKeys,
    /// The turn was cancelled or interrupted while waiting for a key.
    Stopped,
}

/// Take the resource keys this batch's mutating calls will touch, waiting as
/// a visible Blocked state while another agent holds one.
///
/// One acquisition per batch, never nested, so a turn can never hold one key
/// while waiting for another: no hold-and-wait, no deadlock. Every exit from
/// the wait leaves the agent Running again, so the turn ends through its
/// normal path whichever way the wait ended.
pub(super) async fn hold_resources(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    calls: &[(ToolCall, Arc<dyn Tool>)],
    cancel: &CancellationToken,
) -> Result<Held> {
    let mut keys: Vec<ResourceKey> = Vec::new();
    for (call, tool) in calls {
        if !tool.leases_resources(&call.input) {
            continue;
        }
        for key in &call.resources {
            if key.is_leasable() && !keys.contains(key) {
                keys.push(key.clone());
            }
        }
    }
    if keys.is_empty() {
        return Ok(Held::NoKeys);
    }
    // Join the queue before the first attempt: a turn arriving fresh takes
    // its place at the tail rather than barging past an agent already waiting.
    let waiting = runtime
        .resources
        .enter(ctx.agent.id, ctx.agent.config.priority, &keys);
    let conflict = match waiting.try_take() {
        Ok(guard) => {
            runtime.apply(
                Commit::new().event(ctx.draft(EventKind::ResourceAcquired { keys: keys.clone() })),
            )?;
            return Ok(Held::Keys(guard));
        }
        Err(conflict) => conflict,
    };

    block_on_resource(runtime, ctx, &keys, &conflict)?;
    let waited = conflict.describe();
    // Free the slot: the turn is parked, not calling a model.
    set_on_provider(runtime, ctx.agent.id, false);
    runtime.notify.notify_one();
    match waiting
        .wait(cancel, runtime.config.resource_wait_timeout)
        .await
    {
        WaitOutcome::Acquired(guard) => {
            // The park offered this turn's engine slot away, so resuming has
            // to queue for one like a fresh turn - otherwise the cap is only
            // an admission bound and a parked turn overshoots it on wake.
            match retake_engine_slot(runtime, ctx.agent.id, cancel).await {
                Retake::Taken(retaken) => {
                    leave_resource_block(
                        runtime,
                        ctx,
                        Some(&keys),
                        format!("resource free ({waited}){retaken}"),
                    )?;
                    Ok(Held::Keys(guard))
                }
                // Never announced as acquired, so the keys go back silently -
                // and the turn ends, so it takes no slot on the way out.
                Retake::Cancelled => {
                    drop(guard);
                    leave_resource_block(
                        runtime,
                        ctx,
                        None,
                        format!("the turn stopped while {waited}"),
                    )?;
                    Ok(Held::Stopped)
                }
                Retake::TimedOut => {
                    let note = format!(
                        "the tool calls did not run: the daemon stayed at its engine-turn limit \
                         for {} seconds after {waited} ended",
                        engine_retake_wait(runtime).as_secs()
                    );
                    drop(guard);
                    stop_resource_block(runtime, ctx, note)?;
                    Ok(Held::Stopped)
                }
            }
        }
        WaitOutcome::Cancelled => {
            leave_resource_block(
                runtime,
                ctx,
                None,
                format!("the turn stopped while {waited}"),
            )?;
            Ok(Held::Stopped)
        }
        // Bounded like any other wait: the turn ends and the agent is told
        // who held the key, rather than parking out of sight forever.
        WaitOutcome::TimedOut => {
            let note = format!(
                "the tool calls did not run: still {waited} after {} minutes",
                runtime.config.resource_wait_timeout.as_secs() / 60
            );
            stop_resource_block(runtime, ctx, note)?;
            Ok(Held::Stopped)
        }
    }
}

/// End a lease wait that gave up: leave Blocked, and tell the agent why its
/// tool calls did not run so the next turn can decide what to do.
pub(super) fn stop_resource_block(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    note: String,
) -> Result<()> {
    leave_resource_block(runtime, ctx, None, note.clone())?;
    let hold = runtime.hold();
    let mut commit = Commit::new();
    runtime.push_inbox(
        &hold,
        &mut commit,
        &ctx.agent,
        InboxKind::Wake { reason: note },
        Actor::Scheduler,
    )?;
    runtime.apply_held(hold, commit)?;
    Ok(())
}

/// How the queue for an engine slot ended.
pub(crate) enum Retake {
    /// The slot is held again; the note describes the wait, for the resume
    /// reason on the event.
    Taken(&'static str),
    /// The turn was cancelled while it queued: it ends like any other
    /// cancelled wait, with no note and no inbox message.
    Cancelled,
    /// No slot came free in time. The keys go back rather than being sat on.
    TimedOut,
}

/// How long a resuming turn queues for an engine slot before giving its
/// resource keys back. Deliberately far below `resource_wait_timeout`: every agent
/// queued behind those keys is waiting out this queue too, so the turn
/// re-parks as a resource block instead of holding them for the full wait.
pub(super) const ENGINE_RETAKE_WAIT: Duration = Duration::from_secs(60);

/// The bound actually used, never longer than the lease wait itself.
pub(super) fn engine_retake_wait(runtime: &Runtime) -> Duration {
    ENGINE_RETAKE_WAIT.min(runtime.config.resource_wait_timeout)
}

/// Marks the agent as queued for an engine slot for as long as it waits, so
/// `pick` and `claim_link` cannot hand the next free slot to a fresh turn
/// ahead of a waiter that is already holding resource keys.
pub(super) struct EngineRetaker {
    runtime: Arc<Runtime>,
    agent: AgentId,
}

impl EngineRetaker {
    fn queue(runtime: &Arc<Runtime>, agent: AgentId) -> Self {
        runtime.sched.lock().engine_retakers.insert(agent);
        Self {
            runtime: runtime.clone(),
            agent,
        }
    }
}

impl Drop for EngineRetaker {
    fn drop(&mut self) {
        self.runtime
            .sched
            .lock()
            .engine_retakers
            .remove(&self.agent);
    }
}

/// Take an engine slot back for a turn resuming from a park, waiting for one
/// if the daemon is at its cap.
///
/// A turn queueing here holds its resource keys, which is safe because the
/// converse never happens: a turn that parks on a key releases its engine
/// slot first, so no slot holder is ever awaiting a key. It is still a cost
/// to everyone behind those keys, so the wait is bounded by
/// [`ENGINE_RETAKE_WAIT`] rather than by `resource_wait_timeout`.
pub(crate) async fn retake_engine_slot(
    runtime: &Arc<Runtime>,
    agent: AgentId,
    cancel: &CancellationToken,
) -> Retake {
    retake_slot(runtime, agent, cancel, false).await
}

/// [`retake_engine_slot`] for a turn that also gave up its place under the
/// global turn limit, as a native wait does, so that limit, its provider's
/// cap and its supervisors' subtree caps must have room too.
pub(crate) async fn retake_turn_slot(
    runtime: &Arc<Runtime>,
    agent: AgentId,
    cancel: &CancellationToken,
) -> Retake {
    retake_slot(runtime, agent, cancel, true).await
}

/// A turn ending frees a global, provider or subtree slot without notifying
/// `engine_slot`, so a retake that needs one looks again this often.
const GLOBAL_SLOT_RECHECK: Duration = Duration::from_millis(250);

async fn retake_slot(
    runtime: &Arc<Runtime>,
    agent: AgentId,
    cancel: &CancellationToken,
    global: bool,
) -> Retake {
    let cap = runtime.config.max_concurrent_engine_turns;
    let global_cap = runtime.config.max_concurrent_turns;
    debug_assert!(global_cap > 0, "the global limit is validated as positive");
    let deadline = tokio::time::Instant::now() + engine_retake_wait(runtime);
    let provider = global
        .then(|| {
            runtime
                .sched
                .lock()
                .running
                .get(&agent)
                .map(|r| r.provider.clone())
        })
        .flatten();
    let mut queued: Option<EngineRetaker> = None;
    loop {
        // Armed before the check: a slot freed between the two still wakes us.
        let freed = runtime.engine_slot.notified();
        tokio::pin!(freed);
        freed.as_mut().enable();
        let limits = provider
            .as_deref()
            .map(|provider| crate::scheduler::ResumeLimits::load(runtime, provider));
        {
            let mut sched = runtime.sched.lock();
            let needs_slot = sched.running.get(&agent).is_some_and(|r| r.engine);
            let global_free = !global || sched.on_provider().count() < global_cap;
            let limits_free = limits.as_ref().is_none_or(|l| !l.held_back(agent, &sched));
            if global_free && limits_free && (!needs_slot || sched.engine_slot_free(cap)) {
                if let Some(turn) = sched.running.get_mut(&agent) {
                    turn.on_provider = true;
                }
                return Retake::Taken(if queued.is_some() {
                    ", engine slot free"
                } else {
                    ""
                });
            }
        }
        queued.get_or_insert_with(|| EngineRetaker::queue(runtime, agent));
        tokio::select! {
            () = freed => {}
            () = tokio::time::sleep(GLOBAL_SLOT_RECHECK), if global => {}
            () = cancel.cancelled() => return Retake::Cancelled,
            _ = tokio::time::sleep_until(deadline) => return Retake::TimedOut,
        }
    }
}

/// Resource entry/exit owns one held read/write; only committed state reaches the turn.
pub(super) fn update_resource_status(
    runtime: &Runtime,
    ctx: &mut TurnCtx,
    prepare: impl FnOnce(&mut Agent, &TurnCtx) -> Result<Option<Commit>>,
) -> Result<()> {
    let hold = runtime.hold();
    let mut agent = runtime
        .store
        .agent(ctx.agent.id)?
        .ok_or_else(|| RuntimeError::NotFound(ctx.agent.id.to_string()))?;
    let Some(mut commit) = prepare(&mut agent, ctx)? else {
        return Ok(());
    };
    let status = agent.status.clone();
    let status_since = agent.status_since;
    commit.push_mutation(Mutation::UpsertAgent(agent));
    runtime.apply_held(hold, commit)?;
    ctx.agent.status = status;
    ctx.agent.status_since = status_since;
    Ok(())
}

/// Record the wait and move the agent to Blocked, naming the holder.
pub(super) fn block_on_resource(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    keys: &[ResourceKey],
    conflict: &crate::resources::ResourceConflict,
) -> Result<()> {
    update_resource_status(runtime, ctx, |agent, ctx| {
        let reason = BlockReason::Resource {
            holder: conflict.holder,
            key: conflict.key.clone(),
        };
        let described = reason.describe();
        agent.transition(AgentStatus::Blocked { reason }, now())?;
        Ok(Some(
            Commit::new()
                .event(ctx.draft(EventKind::ResourceWaiting {
                    keys: keys.to_vec(),
                    holder: conflict.holder,
                    blocked_on: conflict.key.clone(),
                }))
                .event(ctx.draft(EventKind::AgentBlocked {
                    reason: described,
                    dependency: None,
                })),
        ))
    })
}

/// Say whether this turn is consuming a provider slot right now.
pub(crate) fn set_on_provider(runtime: &Runtime, agent: AgentId, on: bool) {
    let freed_engine = {
        let mut sched = runtime.sched.lock();
        match sched.running.get_mut(&agent) {
            Some(turn) => {
                let freed = !on && turn.on_provider && turn.engine;
                turn.on_provider = on;
                freed
            }
            None => false,
        }
    };
    if freed_engine {
        runtime.engine_slot.notify_waiters();
    }
}

/// End the lease wait and put the agent back in Running, whether the keys
/// came free (`acquired`) or the turn was stopped. The single exit from
/// `block_on_resource`, so no path leaves the agent stranded in Blocked.
///
/// It does not re-take the concurrency slot the park gave away: only
/// `retake_engine_slot` does, gated by the cap. A stopped turn makes no model
/// call before it ends, so it stays off the slot.
pub(super) fn leave_resource_block(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    acquired: Option<&[ResourceKey]>,
    reason: String,
) -> Result<()> {
    update_resource_status(runtime, ctx, |agent, ctx| {
        // Cancelled or failed while it waited: leave the terminal state alone.
        if agent.status.kind() != AgentStatusKind::Blocked {
            return Ok(None);
        }
        agent.transition(AgentStatus::Running, now())?;
        let mut commit =
            Commit::new().event(ctx.draft(EventKind::AgentResumed { reason, via: None }));
        if let Some(keys) = acquired {
            commit.push_event(ctx.draft(EventKind::ResourceAcquired {
                keys: keys.to_vec(),
            }));
        }
        Ok(Some(commit))
    })
}

/// Give the keys back and say so. The guard frees them either way.
pub(super) fn release_resources(
    runtime: &Arc<Runtime>,
    ctx: &TurnCtx,
    guard: Option<ResourceGuard>,
) -> Result<()> {
    let Some(guard) = guard else {
        return Ok(());
    };
    let keys = guard.keys().to_vec();
    drop(guard);
    if !keys.is_empty() {
        runtime.apply(Commit::new().event(ctx.draft(EventKind::ResourceReleased { keys })))?;
    }
    Ok(())
}

pub(super) fn describe_call(call: &ToolCall) -> String {
    let input = serde_json::to_string(&call.input).unwrap_or_default();
    let input = if input.chars().count() > 300 {
        format!("{}…", input.chars().take(300).collect::<String>())
    } else {
        input
    };
    format!("{} {}", call.name, input)
}
