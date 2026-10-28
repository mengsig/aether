use super::*;

/// The agent row a turn writes: the stored row, with the turn's own progress
/// laid over it.
///
/// A turn holds a snapshot taken when it started, so everything a person can
/// change while it runs - the model, the config, the scope, a pause, and the
/// engine session a model change clears - has to come from the store, or the
/// turn writes the operator's decision back away. Status, stats, cursor, the
/// engine session's own counters and the context it built are the turn's own,
/// and nothing else writes them - except a terminal status, which is never
/// written back over.
///
/// The hold is what makes that true: `UpsertAgent` replaces every column, so
/// the read and the commit that writes it back have to be one step. Taking it
/// as an argument makes the compiler carry the requirement to the call sites,
/// the way `push_inbox` does.
pub(crate) fn agent_row(runtime: &Runtime, ctx: &TurnCtx, _hold: &CommitHold<'_>) -> Result<Agent> {
    let mut agent = runtime
        .store
        .agent(ctx.agent.id)?
        .ok_or_else(|| RuntimeError::NotFound(ctx.agent.id.to_string()))?;
    // Something else ended it while the turn ran (a budget, a cancel): that
    // decision stands, exactly as `finish` honors it. Writing the turn's live
    // status back would revive the agent and lose the reason it was stopped
    // for - the spend below is still the turn's own and is written either way.
    if !agent.status.is_terminal() {
        agent.status = ctx.agent.status.clone();
        agent.status_since = ctx.agent.status_since;
    }
    agent.current_task = ctx.agent.current_task;
    agent.inbox_cursor = ctx.agent.inbox_cursor;
    agent.checkpoint = ctx.agent.checkpoint;
    agent.stats = ctx.agent.stats.clone();
    agent.resume_wait.clone_from(&ctx.agent.resume_wait);
    // Tagged with the session it was read from, so a session the store
    // cleared or replaced meanwhile cannot be billed against these counters.
    agent.engine_usage.clone_from(&ctx.agent.engine_usage);
    // Same rule, same reason: the digests are tagged with the session they
    // were sent on, so a replaced session cannot gate the next turn.
    agent.engine_sent.clone_from(&ctx.agent.engine_sent);
    agent.updated_at = ctx.agent.updated_at;
    Ok(agent)
}

/// [`agent_row`] for the two points where the turn itself owns the session:
/// one the engine just opened, and one it refused to resume.
///
/// Only for the engine the agent is still on, unless a handoff is pending.
/// Different explicit models within one native engine share its session;
/// a cross-engine move or a reset to the native default needs a checkpoint.
pub(crate) fn agent_row_with_session(
    runtime: &Runtime,
    ctx: &TurnCtx,
    hold: &CommitHold<'_>,
) -> Result<Agent> {
    let mut agent = agent_row(runtime, ctx, hold)?;
    let same_engine = runtime.engine_of(&ctx.agent.model).is_some()
        && runtime.engine_of(&ctx.agent.model) == runtime.engine_of(&agent.model);
    let handoff_pending = matches!(
        compaction_pending(runtime, agent.id)?,
        Some(CompactionTrigger::Handoff { .. })
    );
    if (agent.model == ctx.agent.model || same_engine) && !handoff_pending {
        agent.engine_session.clone_from(&ctx.agent.engine_session);
    }
    Ok(agent)
}

/// Record the turn's end and act on it, in one go; the turn itself waits out
/// a locked database between the two (`execute`).
#[cfg(test)]
pub(super) fn finish(runtime: &Runtime, ctx: &TurnCtx, next: Next) -> Result<()> {
    match record_end(runtime, ctx, next)? {
        Some(ended) => after_end(runtime, ctx, ended),
        None => Ok(()),
    }
}

/// What a recorded end leaves to do once it is durable.
pub(super) struct Ended {
    reap: Option<crate::commands::PendingReap>,
    bench: Option<(String, BlockReason)>,
    cancelled: bool,
}

/// Commit the turn's end: the agent's next status, the lease released. One
/// atomic commit with no effect until it lands, so a refused one can simply
/// be recorded again. `None` when the turn's end is not this turn's to write.
pub(super) fn record_end(runtime: &Runtime, ctx: &TurnCtx, next: Next) -> Result<Option<Ended>> {
    // Above the read: the whole row is written back below, so an `update_agent`
    // landing between the two would be silently reverted.
    let hold = runtime.hold();
    let mut agent = runtime
        .store
        .agent(ctx.agent.id)?
        .ok_or_else(|| RuntimeError::NotFound(ctx.agent.id.to_string()))?;
    // Something else ended it while the turn ran (a budget, a cancel): that
    // decision stands.
    if agent.status.is_terminal() {
        return Ok(None);
    }
    let at = now();
    // Whatever happens next, the interrupted wait was either restored
    // explicitly below or superseded by the agent's own decision.
    agent.resume_wait = None;
    let invocation = latest_invocation_id(runtime, ctx.agent.id)?;
    let mut commit = Commit::new();
    let mut reap = None;
    let mut bench = None;
    let cancelled = matches!(next, Next::Cancelled(_));
    // Asked for while the turn ran: fold now rather than at the next message.
    let owed_by = match compaction_pending(runtime, ctx.agent.id)? {
        Some(CompactionTrigger::Explicit { by }) => Some(by),
        _ => None,
    };
    match next {
        Next::Ready => {
            agent.transition(AgentStatus::Ready, at)?;
            commit.push_mutation(Mutation::ClearLease(agent.id));
            commit.push_event(ctx.draft(EventKind::LeaseReleased));
            commit.push_event(ctx.draft(EventKind::AgentTurnCompleted {
                turn: ctx.turn,
                invocation,
                next: AgentStatusKind::Ready,
            }));
            commit.push_mutation(Mutation::UpsertAgent(agent));
        }
        Next::Waiting(condition) => {
            let condition = match runtime.config.wait_timeout {
                Some(timeout) => condition.with_deadline(deadline_after(timeout)),
                None => condition,
            };
            agent.transition(
                AgentStatus::Waiting {
                    condition: condition.clone(),
                },
                at,
            )?;
            commit.push_mutation(Mutation::ClearLease(agent.id));
            commit.push_event(ctx.draft(EventKind::LeaseReleased));
            commit.push_event(ctx.draft(EventKind::AgentWaiting { condition }));
            commit.push_event(ctx.draft(EventKind::AgentTurnCompleted {
                turn: ctx.turn,
                invocation,
                next: AgentStatusKind::Waiting,
            }));
            match owed_by {
                Some(by) => runtime.ready_for_compaction(&hold, &mut commit, &mut agent, by)?,
                None => commit.push_mutation(Mutation::UpsertAgent(agent)),
            }
        }
        Next::Completed(summary) => {
            commit.push_event(ctx.draft(EventKind::AgentTurnCompleted {
                turn: ctx.turn,
                invocation,
                next: AgentStatusKind::Completed,
            }));
            reap = Some(runtime.terminate_agent(
                &hold,
                &mut commit,
                agent,
                AgentStatus::Completed { summary },
                ctx.base_actor.clone(),
            )?);
        }
        Next::Failed(error) => {
            commit.push_event(ctx.draft(EventKind::AgentTurnCompleted {
                turn: ctx.turn,
                invocation,
                next: AgentStatusKind::Failed,
            }));
            reap = Some(runtime.terminate_agent(
                &hold,
                &mut commit,
                agent,
                AgentStatus::Failed { error },
                ctx.base_actor.clone(),
            )?);
        }
        Next::Interrupted => {
            let condition = WaitCondition::Inbox;
            agent.transition(
                AgentStatus::Waiting {
                    condition: condition.clone(),
                },
                at,
            )?;
            commit.push_mutation(Mutation::ClearLease(agent.id));
            commit.push_event(ctx.draft(EventKind::LeaseReleased));
            commit.push_event(ctx.draft(EventKind::AgentInterrupted { turn: ctx.turn }));
            commit.push_event(ctx.draft(EventKind::AgentWaiting { condition }));
            commit.push_event(ctx.draft(EventKind::AgentTurnCompleted {
                turn: ctx.turn,
                invocation,
                next: AgentStatusKind::Waiting,
            }));
            // An operator stop stands: an owed fold waits for the next turn.
            commit.push_mutation(Mutation::UpsertAgent(agent));
        }
        Next::Suspended(cause) => {
            // §5 fence: only a turn whose row is still Running suspends it. A
            // replay or a racing attempt sees Blocked and writes nothing, so
            // the ancestors are told once and the episode keeps its schedule.
            if agent.status.kind() != AgentStatusKind::Running {
                return Ok(None);
            }
            let suspension = crate::exhaustion::plan(runtime, &agent, &cause)?;
            agent.transition(
                AgentStatus::Blocked {
                    reason: suspension.reason.clone(),
                },
                at,
            )?;
            commit.push_mutation(Mutation::ClearLease(agent.id));
            commit.push_event(ctx.draft(EventKind::LeaseReleased));
            commit.push_event(ctx.draft(EventKind::AgentBlocked {
                reason: suspension.reason.describe(),
                dependency: None,
            }));
            crate::exhaustion::record(
                runtime,
                &hold,
                &mut commit,
                &agent,
                &cause,
                &suspension,
                &ctx.base_actor,
            )?;
            commit.push_event(ctx.draft(EventKind::AgentTurnCompleted {
                turn: ctx.turn,
                invocation,
                next: AgentStatusKind::Blocked,
            }));
            commit.push_mutation(Mutation::UpsertAgent(agent));
            bench = Some((cause.model.provider.clone(), suspension.reason));
        }
        Next::Cancelled(reason) => {
            let request = runtime
                .sched
                .lock()
                .running
                .get(&ctx.agent.id)
                .and_then(|running| running.cancel_request.clone());
            // Only the daemon stopping ended the turn: leave it Running under
            // its lease, as a crash would, so the next start recovers it.
            if request.is_none() && runtime.shutdown.is_cancelled() {
                return Ok(None);
            }
            let (by, reason) = request.map_or((Actor::System, reason), |request| {
                (request.by, request.reason)
            });
            commit.push_event(ctx.draft(EventKind::AgentTurnCompleted {
                turn: ctx.turn,
                invocation,
                next: AgentStatusKind::Cancelled,
            }));
            reap = Some(runtime.terminate_agent(
                &hold,
                &mut commit,
                agent,
                AgentStatus::Cancelled { reason },
                by,
            )?);
        }
    }
    runtime.apply_held(hold, commit)?;
    Ok(Some(Ended {
        reap,
        bench,
        cancelled,
    }))
}

/// The steps that follow a durable end: reaping, benching a spent provider,
/// and repeating a cancellation down the tree.
pub(super) fn after_end(runtime: &Runtime, ctx: &TurnCtx, ended: Ended) -> Result<()> {
    let Ended {
        reap,
        bench,
        cancelled,
    } = ended;
    // Only once the agent is durably terminal: its processes outlive nothing.
    if let Some(reap) = reap {
        runtime.reap_ended_agent(reap);
    }
    // Only once the suspension is durable: the bench holds every other agent
    // off a spent provider until this one has probed it. A longer hold already
    // on the provider stands - `backoff` keeps the later expiry - and the probe
    // then waits behind it, which costs time but never gives the agent up.
    if let Some((
        provider,
        BlockReason::Exhausted {
            class, next_probe, ..
        },
    )) = bench
    {
        let wait = (next_probe - aether_core::time::now())
            .to_std()
            .unwrap_or_default();
        crate::scheduler::backoff(
            runtime,
            &provider,
            format!("{} from the provider", class.as_str()),
            wait,
        )?;
    }
    if (cancelled || ctx.hard_cancel.is_cancelled()) && !runtime.shutdown.is_cancelled() {
        // Repeat after this turn settles. The durable cancellation journal
        // retains the request if any descendant is still running or this walk
        // fails. A daemon shutdown alone must not cancel otherwise idle children.
        let by = runtime
            .sched
            .lock()
            .running
            .get(&ctx.agent.id)
            .and_then(|running| {
                running
                    .cancel_request
                    .as_ref()
                    .map(|request| request.by.clone())
            })
            .unwrap_or(Actor::System);
        runtime.cancel_live_children(ctx.agent.id, by)?;
    }
    Ok(())
}

/// The link that answered ends the exhaustion episode it was in, if any. A
/// fallback answering ends nothing: the episode belongs to the spent link.
/// Committed with the invocation that answered, under the same hold,
/// and only for the first completed invocation on that link. `pub(crate)`: the
/// engine path commits its own invocations.
pub(crate) fn close_exhaustion_episode(
    runtime: &Runtime,
    ctx: &mut TurnCtx,
    hold: &CommitHold<'_>,
    commit: &mut Commit,
    answered: &ModelRef,
) -> Result<()> {
    if ctx.recovery_checked_for.as_ref() == Some(answered) {
        return Ok(());
    }
    ctx.recovery_checked_for = Some(answered.clone());
    let Some(provider) = crate::exhaustion::record_recovery(
        runtime,
        hold,
        commit,
        &ctx.agent,
        answered,
        &ctx.base_actor,
    )?
    else {
        return Ok(());
    };
    // The claim guarded a provider that had said no; it has now said yes, so
    // the rest of this turn is no reason to keep everyone else waiting.
    crate::scheduler::release_probe_claim_of(&mut runtime.sched.lock(), ctx.agent.id);
    // Everyone else parked on this provider may try now; the tick still lets
    // them through one at a time.
    crate::scheduler::provider_recovered_by_probe(runtime, hold, commit, &provider, ctx.agent.id)
}

pub(super) fn deadline_after(wait: Duration) -> Timestamp {
    now() + chrono::Duration::from_std(wait).unwrap_or_else(|_| chrono::Duration::zero())
}
