//! Restart recovery: classify every interrupted turn and put the agent into
//! a state the scheduler can reason about.

use std::sync::Arc;

use aether_core::agent::{AgentStatus, AgentStatusKind, BlockReason};
use aether_core::event::{Actor, EventDraft, EventKind, RecoveryClass};
use aether_core::id::{AgentId, ToolCallId};
use aether_core::time::now;
use aether_core::tool::{ResourceKey, SideEffectClass};
use aether_store::{AgentFilter, Commit, EventFilter, Mutation};

use crate::{Result, Runtime};

/// Updated only after recovery succeeds: a crash midway through the pass must
/// leave the previous boot available to the next attempt.
const BOOT_ID_META: &str = "runtime.daemon_boot_id";

fn host_boot_id() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        match std::fs::read_to_string("/proc/sys/kernel/random/boot_id") {
            Ok(id) if !id.trim().is_empty() => Some(id.trim().to_owned()),
            Ok(_) => {
                tracing::warn!("empty host boot id; keeping conservative restart recovery");
                None
            }
            Err(error) => {
                tracing::warn!(%error, "cannot read host boot id; keeping conservative restart recovery");
                None
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    None
}

pub(crate) fn recover(runtime: &Arc<Runtime>) -> Result<()> {
    recover_with_boot_id(runtime, host_boot_id().as_deref())
}

fn host_restarted(previous: Option<&str>, current: Option<&str>) -> bool {
    matches!((previous, current), (Some(previous), Some(current))
        if !previous.is_empty() && !current.is_empty() && previous != current)
}

fn recover_with_boot_id(runtime: &Arc<Runtime>, current_boot: Option<&str>) -> Result<()> {
    let previous_boot = runtime.store.meta(BOOT_ID_META)?;
    let rebooted = host_restarted(previous_boot.as_deref(), current_boot);
    let mut stale = runtime.store.leases()?;
    // Agents marked running without a lease are just as interrupted.
    for agent in runtime.store.agents(&AgentFilter {
        statuses: vec![AgentStatusKind::Running],
        ..Default::default()
    })? {
        if !stale.iter().any(|l| l.agent_id == agent.id) {
            stale.push(aether_core::lease::Lease::new(
                agent.id,
                "unknown",
                agent.stats.turns,
                now(),
                std::time::Duration::ZERO,
            ));
        }
    }
    let quarantined = recover_leases(runtime, stale, rebooted)?;
    requarantine_inspected(runtime, &quarantined, rebooted)?;
    runtime.apply(Commit::new().mutate(Mutation::SetMeta {
        key: BOOT_ID_META.into(),
        // Forget an unavailable boot rather than comparing a later boot with
        // an older daemon whose processes are no longer the ones at risk.
        value: current_boot.unwrap_or_default().into(),
    }))?;
    Ok(())
}

/// Classify each stale lease's turn and put its agent where that leaves it.
/// `rebooted` only when the host boot changed since the last recovery.
/// Returns the agents parked for inspection.
pub(crate) fn recover_leases(
    runtime: &Arc<Runtime>,
    stale: Vec<aether_core::lease::Lease>,
    rebooted: bool,
) -> Result<Vec<AgentId>> {
    let mut quarantined: Vec<AgentId> = Vec::new();
    let cancelling = crate::cancellation::pending_agents(runtime)?;
    for lease in stale {
        // The scheduler retries durable cancellations before dispatching any work.
        if cancelling.contains(&lease.agent_id) {
            continue;
        }
        let Some(mut agent) = runtime.store.agent(lease.agent_id)? else {
            runtime.apply(Commit::new().mutate(Mutation::ClearLease(lease.agent_id)))?;
            continue;
        };
        let found = classify(runtime, &agent, rebooted)?;
        let (class, detail) = (found.class, found.detail.clone());
        // After a reboot nothing can still be writing: what inspection would
        // fence off, including cancelled calls' keys, is released instead.
        let held = if class == RecoveryClass::Inspect || rebooted {
            let mut keys = unreleased_keys(runtime, agent.id)?;
            keys.extend(found.cancelled_keys.iter().cloned());
            keys.sort();
            keys.dedup();
            keys
        } else {
            Vec::new()
        };
        let at = now();
        let hold = runtime.hold();
        let mut commit = Commit::new();
        let (aid, parent, run) = (agent.id, agent.parent, agent.run);
        let base = move |kind: EventKind| {
            EventDraft::new(Actor::System, kind)
                .agent(aid)
                .parent(parent)
                .run(run)
        };
        commit.push_event(base(EventKind::LeaseExpired {
            holder: lease.holder.clone(),
        }));
        commit.push_mutation(Mutation::ClearLease(agent.id));
        for call in &found.open_calls {
            crate::tool_interruption::record(
                &mut commit,
                &agent,
                *call,
                found
                    .calls
                    .get(call)
                    .map_or("unknown tool", |call| call.name.as_str()),
                &found
                    .calls
                    .get(call)
                    .map_or(serde_json::Value::Null, |call| call.input.clone()),
                &format!("recovery.classified ({class:?}): {detail}"),
            );
        }
        if let Some((invocation, snapshot)) = found.open_invocation {
            commit.push_event(base(EventKind::InvocationFailed {
                class: None,
                invocation,
                snapshot,
                error: "daemon restarted during the turn".into(),
                retryable: true,
                cancelled: false,
                partial: Vec::new(),
            }));
        }
        commit.push_event(base(EventKind::RecoveryClassified {
            class,
            detail: detail.clone(),
        }));
        if class != RecoveryClass::Inspect && !held.is_empty() {
            commit.push_event(base(EventKind::ResourceReleased { keys: held.clone() }));
        }
        if agent.status.is_terminal() {
            commit.push_mutation(Mutation::UpsertAgent(agent));
            runtime.apply_held(hold, commit)?;
            continue;
        }
        let next = match class {
            RecoveryClass::Resume | RecoveryClass::Retry => AgentStatus::Ready,
            RecoveryClass::Inspect => AgentStatus::Blocked {
                reason: BlockReason::Inspection {
                    detail: detail.clone(),
                },
            },
        };
        // A turn waiting for a resource key is interrupted like any other: the
        // lease table died with the daemon, so the wait cannot be resumed and
        // the classification decides where the agent goes. A dependency gate,
        // an inspection or an operator pause is not this pass's business.
        let interrupted = matches!(
            agent.status.kind(),
            AgentStatusKind::Running | AgentStatusKind::Ready
        ) || matches!(
            &agent.status,
            AgentStatus::Blocked {
                reason: BlockReason::Resource { .. }
            }
        );
        if interrupted {
            if agent.status.kind() != next.kind() || agent.status != next {
                agent.transition(next.clone(), at)?;
            }
            match &next {
                AgentStatus::Blocked { reason } => {
                    commit.push_event(base(EventKind::AgentBlocked {
                        reason: reason.describe(),
                        dependency: reason.dependency(),
                    }));
                    // VISION §15: the tool process that took these keys may
                    // have outlived the daemon and still be writing. Nobody
                    // else takes them until a person clears the inspection.
                    if !held.is_empty() {
                        commit.push_event(base(EventKind::ResourceAcquired { keys: held.clone() }));
                    }
                }
                _ => commit.push_event(base(EventKind::AgentResumed {
                    reason: "recovered after restart".into(),
                    via: None,
                })),
            }
            // Ready alone does not make the agent run: a turn interrupted
            // after its last assistant message reads as answered. A retry
            // needs the wake that says otherwise, once per restart.
            if class == RecoveryClass::Retry {
                runtime.push_inbox(
                    &hold,
                    &mut commit,
                    &agent,
                    aether_core::inbox::InboxKind::Wake {
                        reason: detail.clone(),
                    },
                    Actor::System,
                )?;
            }
        }
        commit.push_mutation(Mutation::UpsertAgent(agent));
        runtime.apply_held(hold, commit)?;
        // Only once the acquisition is in the log: a hold no event explains
        // is a key nobody can account for.
        if class == RecoveryClass::Inspect && !held.is_empty() {
            runtime.resources.quarantine(aid, held);
        }
        if class == RecoveryClass::Inspect {
            quarantined.push(aid);
        }
    }
    Ok(quarantined)
}

/// The inspection hold outlives daemon restarts on the same host boot.
/// `ResourceLeases` is in memory and starts empty, so an agent already parked
/// for inspection - which recovery's own pass no longer visits, its lease
/// cleared and its status no longer Running - would come back holding
/// nothing while §15 says the keys stay fenced off until a person looks.
/// The log's unmatched acquisitions are the only record left of them.
/// `done` names the agents the pass above already walked and quarantined -
/// including the one it just parked - so no agent's history is read twice.
fn requarantine_inspected(runtime: &Arc<Runtime>, done: &[AgentId], rebooted: bool) -> Result<()> {
    for mut agent in runtime.store.agents(&AgentFilter {
        statuses: vec![AgentStatusKind::Blocked],
        ..Default::default()
    })? {
        if done.contains(&agent.id) {
            continue;
        }
        let AgentStatus::Blocked {
            reason: BlockReason::Inspection { detail },
        } = &agent.status
        else {
            continue;
        };
        let held = unreleased_keys(runtime, agent.id)?;
        if rebooted {
            // These calls were cancelled in the log when inspection began,
            // so classify() can no longer find them open. Keep the inspection
            // detail in the wake to identify the original calls.
            let detail = reboot_detail(detail);
            let hold = runtime.hold();
            let base = |kind| {
                EventDraft::new(Actor::System, kind)
                    .agent(agent.id)
                    .parent(agent.parent)
                    .run(agent.run)
            };
            let mut commit = Commit::new()
                .event(base(EventKind::RecoveryClassified {
                    class: RecoveryClass::Retry,
                    detail: detail.clone(),
                }))
                .event(base(EventKind::AgentResumed {
                    reason: "inspection cleared after host restart".into(),
                    via: None,
                }));
            if !held.is_empty() {
                commit.push_event(base(EventKind::ResourceReleased { keys: held }));
            }
            agent.transition(AgentStatus::Ready, now())?;
            runtime.push_inbox(
                &hold,
                &mut commit,
                &agent,
                aether_core::inbox::InboxKind::Wake { reason: detail },
                Actor::System,
            )?;
            commit.push_mutation(Mutation::UpsertAgent(agent));
            runtime.apply_held(hold, commit)?;
        } else if !held.is_empty() {
            runtime.resources.quarantine(agent.id, held);
        }
    }
    Ok(())
}

/// What recovery found in an interrupted turn.
struct Interrupted {
    calls: std::collections::BTreeMap<ToolCallId, aether_core::tool::ToolCall>,
    class: RecoveryClass,
    detail: String,
    open_calls: Vec<ToolCallId>,
    cancelled_keys: Vec<ResourceKey>,
    open_invocation: Option<(aether_core::id::InvocationId, aether_core::id::SnapshotId)>,
}

/// Events the agent's own turn wrote, oldest first, read a page at a time.
/// The walk is bounded by the turn, never by a count: a long or busy turn
/// writes lease renewals and tool events without limit, and a window that
/// loses the turn start reads an interrupted mutating call as idle.
const TURN_EVENT_PAGE: usize = 500;

/// Visit the agent's own events with `seq > after`, oldest first, narrowed to
/// `types` when it is non-empty. Its children's are left out: only what this
/// agent did decides what it left open.
fn walk_events(
    runtime: &Runtime,
    agent: AgentId,
    after: u64,
    types: Vec<String>,
    mut visit: impl FnMut(&EventKind),
) -> Result<()> {
    let filter = EventFilter {
        types,
        ..EventFilter::own_events(agent)
    };
    let mut after = after;
    loop {
        let page = runtime.store.events(after, TURN_EVENT_PAGE, &filter)?;
        let Some(last) = page.last() else { break };
        after = last.seq;
        for event in &page {
            visit(&event.kind);
        }
        if page.len() < TURN_EVENT_PAGE {
            break;
        }
    }
    Ok(())
}

/// Keys the agent took and never released, over its whole history. Recovery
/// records its own quarantine as an acquisition too, so a key can be taken
/// twice and is counted once.
fn unreleased_keys(runtime: &Runtime, agent: AgentId) -> Result<Vec<ResourceKey>> {
    let mut held: Vec<ResourceKey> = Vec::new();
    // The whole history, so the two lease kinds are all it may read: a
    // supervisor's unfiltered pages carry every child's events too, and this
    // runs per parked agent on every daemon start.
    let types = vec![
        "lease.resource_acquired".to_owned(),
        "lease.resource_released".to_owned(),
    ];
    walk_events(runtime, agent, 0, types, |kind| match kind {
        EventKind::ResourceAcquired { keys } => held.extend(keys.iter().cloned()),
        EventKind::ResourceReleased { keys } => held.retain(|k| !keys.contains(k)),
        _ => {}
    })?;
    held.sort();
    held.dedup();
    Ok(held)
}

/// The seq the agent's current turn began at, or 0 when it never started one.
fn turn_start_seq(runtime: &Runtime, agent: AgentId) -> Result<u64> {
    Ok(runtime
        .store
        .latest_events(
            1,
            &EventFilter {
                types: vec!["agent.started".into()],
                ..EventFilter::own_events(agent)
            },
        )?
        .first()
        .map(|e| e.seq)
        .unwrap_or(0))
}

/// Look at the events since the agent's last turn start and decide.
fn classify(
    runtime: &Runtime,
    agent: &aether_core::agent::Agent,
    rebooted: bool,
) -> Result<Interrupted> {
    // Open work as the turn left it. Walking forward, each event either opens
    // something or closes what an earlier one opened.
    let mut calls: std::collections::BTreeMap<
        ToolCallId,
        (aether_core::tool::ToolCall, SideEffectClass),
    > = Default::default();
    let mut open_tools: std::collections::BTreeSet<ToolCallId> = Default::default();
    let mut invocation_open: Option<(aether_core::id::InvocationId, aether_core::id::SnapshotId)> =
        None;
    let mut cancelled_invocation = false;
    let mut cancelled_tools: std::collections::BTreeSet<ToolCallId> = Default::default();
    let start = turn_start_seq(runtime, agent.id)?;
    walk_events(runtime, agent.id, start, Vec::new(), |kind| match kind {
        EventKind::ToolRequested { call, side_effect } => {
            calls.insert(call.id, (call.clone(), *side_effect));
        }
        EventKind::ToolStarted { call } => {
            open_tools.insert(*call);
        }
        EventKind::ToolCancelled { call } => {
            if open_tools.remove(call) {
                cancelled_tools.insert(*call);
            }
        }
        EventKind::ToolSucceeded { call, .. }
        | EventKind::ToolFailed { call, .. }
        | EventKind::ToolDenied { call, .. } => {
            open_tools.remove(call);
            cancelled_tools.remove(call);
        }
        EventKind::InvocationFailed { cancelled, .. } => {
            invocation_open = None;
            cancelled_invocation |= *cancelled;
        }
        EventKind::InvocationCompleted { .. } => invocation_open = None,
        EventKind::SnapshotCreated { snapshot, .. } => {
            // The invocation id is minted at send time; a synthetic one is
            // fine because the failed record only needs the snapshot link.
            invocation_open = Some((aether_core::id::InvocationId::new(), *snapshot));
        }
        _ => {}
    })?;
    let open_calls: Vec<ToolCallId> = open_tools.into_iter().collect();
    let mut needs_inspection = Vec::new();
    let mut cancelled_keys = Vec::new();
    // Cancellation stops execution but cannot roll back effects already applied.
    for call in open_calls.iter().chain(&cancelled_tools) {
        let requested = calls.get(call);
        let safe = requested.is_some_and(|(requested, recorded)| {
            recovery_side_effect(runtime, &requested.name, *recorded).safe_to_retry()
        });
        if safe {
            continue;
        }
        let name = requested.map_or("unknown tool", |(requested, _)| requested.name.as_str());
        needs_inspection.push(format!("{name} ({call})"));
        if cancelled_tools.contains(call)
            && let Some((requested, _)) = requested
        {
            cancelled_keys.extend(
                requested
                    .resources
                    .iter()
                    .filter(|key| key.is_leasable())
                    .cloned(),
            );
        }
    }
    let (class, detail) = if rebooted && !needs_inspection.is_empty() {
        (
            RecoveryClass::Retry,
            reboot_detail(&needs_inspection.join(", ")),
        )
    } else if !needs_inspection.is_empty() {
        (
            RecoveryClass::Inspect,
            format!(
                "mutating tool calls were in flight when the daemon stopped: {}",
                needs_inspection.join(", ")
            ),
        )
    } else if !open_calls.is_empty()
        || invocation_open.is_some()
        || !cancelled_tools.is_empty()
        || cancelled_invocation
    {
        (
            RecoveryClass::Retry,
            "turn interrupted; safe to retry from the last committed step".into(),
        )
    } else if agent.status.kind() == AgentStatusKind::Running {
        // An engine keeps the turn inside its own process and commits nothing
        // between the start and the reply, so the log alone reads as idle.
        // The Running status is the evidence that a turn was lost.
        (
            RecoveryClass::Retry,
            "the turn in flight was lost when the daemon stopped; nothing it produced was committed"
                .into(),
        )
    } else {
        (RecoveryClass::Resume, "no work in flight".into())
    };
    Ok(Interrupted {
        calls: calls
            .into_iter()
            .map(|(id, (call, _))| (id, call))
            .collect(),
        class,
        detail,
        open_calls,
        cancelled_keys,
        open_invocation: invocation_open,
    })
}

/// Native edits can be idempotent once complete, but retrying alongside a
/// surviving writer is unsafe. Older adapters also labelled every unknown
/// Aether-prefixed tool read-only; only its registered descriptor is trusted.
fn recovery_side_effect(
    runtime: &Runtime,
    name: &str,
    recorded: SideEffectClass,
) -> SideEffectClass {
    if let Some(bare) = name
        .strip_prefix("mcp__aether__")
        .or_else(|| name.strip_prefix("mcp__aether."))
        .or_else(|| name.strip_prefix("aether__"))
        .or_else(|| name.strip_prefix("aether."))
    {
        return runtime
            .tools
            .get(bare)
            .map(|tool| tool.descriptor().side_effect)
            .unwrap_or(SideEffectClass::Mutation);
    }
    // Exact registry names are the API path too; its recorded contract must
    // survive a descriptor change between the call and the restart.
    if runtime.tools.get(name).is_some() {
        return recorded;
    }
    match name.to_ascii_lowercase().as_str() {
        "read" | "grep" | "glob" | "ls" | "list_dir" | "toolsearch" | "websearch" | "webfetch"
        | "todowrite" => SideEffectClass::ReadOnly,
        "bash" | "edit" | "write" | "multiedit" | "notebookedit" | "write_file" | "edit_file"
        | "apply_patch" | "file_change" => SideEffectClass::Mutation,
        _ => recorded,
    }
}

fn reboot_detail(calls: &str) -> String {
    format!(
        "the host restarted while these calls were running: {calls}; their local processes were killed; side effects may be partial, including requests already sent to external services; re-check state before redoing them"
    )
}

#[cfg(test)]
mod tests;
