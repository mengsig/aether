//! Restart recovery: classify every interrupted turn and put the agent into
//! a state the scheduler can reason about.

use std::sync::Arc;

mod validation;

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
    let cancelling = crate::cancellation::pending_agents(runtime)?;
    for record in runtime
        .store
        .quarantines()?
        .into_iter()
        .filter(|q| q.table == "leases" || q.table == "agents")
    {
        if let Ok(id) = record.id.parse::<AgentId>() {
            if cancelling.contains(&id) {
                continue;
            }
            validation::quarantine_failed_recovery(
                runtime,
                id,
                &crate::RuntimeError::Invalid(record.reason),
            )?;
            stale.retain(|lease| lease.agent_id != id);
        }
    }
    let recovered = recover_leases(runtime, stale, rebooted, None)?;
    requarantine_inspected(runtime, &recovered.quarantined, rebooted)?;
    validation::validate_live(runtime)?;
    runtime.apply(Commit::new().mutate(Mutation::SetMeta {
        key: BOOT_ID_META.into(),
        // Forget an unavailable boot rather than comparing a later boot with
        // an older daemon whose processes are no longer the ones at risk.
        value: current_boot.unwrap_or_default().into(),
    }))?;
    Ok(())
}

/// What [`recover_leases`] did with the agents it was given.
#[derive(Debug, Default)]
pub(crate) struct Recovered {
    /// Parked for inspection.
    pub(crate) quarantined: Vec<AgentId>,
    /// Left alone: written to while recovery read them, so no longer as read.
    pub(crate) changed: Vec<AgentId>,
}

/// Classify each stale lease's turn and put its agent where that leaves it.
/// `rebooted` only when the host boot changed since the last recovery.
/// `reason`, when given, is recorded with each recovery as `agent.recovered`.
pub(crate) fn recover_leases(
    runtime: &Arc<Runtime>,
    stale: Vec<aether_core::lease::Lease>,
    rebooted: bool,
    reason: Option<&str>,
) -> Result<Recovered> {
    let mut done = Recovered::default();
    let cancelling = crate::cancellation::pending_agents(runtime)?;
    for lease in stale {
        if cancelling.contains(&lease.agent_id) {
            continue;
        }
        let id = lease.agent_id;
        let recovered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            recover_lease(runtime, lease, rebooted, reason)
        }))
        .unwrap_or_else(|panic| {
            Err(crate::RuntimeError::Invalid(format!(
                "recovery panicked for {id}: {}; run `aether doctor`",
                crate::turn::panic_message(panic.as_ref())
            )))
        });
        match recovered {
            Ok(Outcome::Quarantined) => done.quarantined.push(id),
            Ok(Outcome::Changed) => done.changed.push(id),
            Ok(Outcome::Done) => {}
            Err(error) => {
                validation::quarantine_failed_recovery(runtime, id, &error)?;
                done.quarantined.push(id);
            }
        }
    }
    Ok(done)
}

/// How one agent's recovery ended.
#[derive(Debug, PartialEq)]
pub(crate) enum Outcome {
    Done,
    Quarantined,
    Changed,
}

fn recover_lease(
    runtime: &Arc<Runtime>,
    lease: aether_core::lease::Lease,
    rebooted: bool,
    reason: Option<&str>,
) -> Result<Outcome> {
    let Some(agent) = runtime.store.agent(lease.agent_id)? else {
        runtime.apply(Commit::new().mutate(Mutation::ClearLease(lease.agent_id)))?;
        return Ok(Outcome::Done);
    };
    recover_agent(runtime, lease, agent, rebooted, reason)
}

/// Recover `agent` as read before the commit lock. A live daemon may write it
/// meanwhile (a cancel, a pause); then the read is stale and it is left alone.
pub(crate) fn recover_agent(
    runtime: &Arc<Runtime>,
    lease: aether_core::lease::Lease,
    mut agent: aether_core::agent::Agent,
    rebooted: bool,
    reason: Option<&str>,
) -> Result<Outcome> {
    debug_assert_eq!(lease.agent_id, agent.id, "the lease is the agent's");
    let found = classify_or_inspect(runtime, &agent, rebooted)?;
    let held = if found.class == RecoveryClass::Inspect || rebooted {
        let mut keys = unreleased_keys(runtime, agent.id)?;
        keys.extend(found.cancelled_keys.iter().cloned());
        keys.sort();
        keys.dedup();
        keys
    } else {
        Vec::new()
    };
    let hold = runtime.hold();
    if runtime.store.agent(agent.id)?.as_ref() != Some(&agent) {
        tracing::info!(agent = %agent.id, "agent changed during recovery; leaving it");
        return Ok(Outcome::Changed);
    }
    let mut commit = interruption_commit(&agent, &lease, &found, &held);
    if let Some(reason) = reason {
        commit.push_event(crate::orphans::recovered_event(&agent, reason));
    }
    let terminal = agent.status.is_terminal();
    if !terminal {
        resume_recovered(runtime, &hold, &mut commit, &mut agent, &found, &held)?;
    }
    let id = agent.id;
    commit.push_mutation(Mutation::UpsertAgent(agent));
    runtime.apply_held(hold, commit)?;
    if terminal || found.class != RecoveryClass::Inspect {
        return Ok(Outcome::Done);
    }
    // Quarantine only after its event commits: no unexplained in-memory hold.
    if !held.is_empty() {
        runtime.resources.quarantine(id, held);
    }
    Ok(Outcome::Quarantined)
}

fn recovery_event(agent: &aether_core::agent::Agent, kind: EventKind) -> EventDraft {
    EventDraft::new(Actor::System, kind)
        .agent(agent.id)
        .parent(agent.parent)
        .run(agent.run)
}

fn interruption_commit(
    agent: &aether_core::agent::Agent,
    lease: &aether_core::lease::Lease,
    found: &Interrupted,
    held: &[ResourceKey],
) -> Commit {
    let mut commit = Commit::new()
        .event(recovery_event(
            agent,
            EventKind::LeaseExpired {
                holder: lease.holder.clone(),
            },
        ))
        .mutate(Mutation::ClearLease(agent.id));
    record_open_calls(&mut commit, agent, found);
    if let Some((invocation, snapshot)) = found.open_invocation {
        commit.push_event(recovery_event(agent, EventKind::InvocationFailed {
            class: None, invocation, snapshot,
            error: "daemon restarted during the turn; run `aether doctor` if recovery needs attention".into(),
            retryable: true, cancelled: false, partial: Vec::new(),
        }));
    }
    commit.push_event(recovery_event(
        agent,
        EventKind::RecoveryClassified {
            class: found.class,
            detail: found.detail.clone(),
        },
    ));
    if found.class != RecoveryClass::Inspect && !held.is_empty() {
        commit.push_event(recovery_event(
            agent,
            EventKind::ResourceReleased {
                keys: held.to_vec(),
            },
        ));
    }
    commit
}

fn record_open_calls(commit: &mut Commit, agent: &aether_core::agent::Agent, found: &Interrupted) {
    for id in &found.open_calls {
        let call = found.calls.get(id);
        crate::tool_interruption::record(
            commit,
            agent,
            *id,
            call.map_or("unknown tool", |call| call.name.as_str()),
            &call.map_or(serde_json::Value::Null, |call| call.input.clone()),
            &format!("recovery.classified ({:?}): {}", found.class, found.detail),
        );
    }
}

fn resume_recovered(
    runtime: &Runtime,
    hold: &crate::CommitHold<'_>,
    commit: &mut Commit,
    agent: &mut aether_core::agent::Agent,
    found: &Interrupted,
    held: &[ResourceKey],
) -> Result<()> {
    let interrupted = matches!(
        agent.status.kind(),
        AgentStatusKind::Running | AgentStatusKind::Ready
    ) || matches!(
        &agent.status,
        AgentStatus::Blocked {
            reason: BlockReason::Resource { .. }
        }
    );
    if !interrupted {
        return Ok(());
    }
    let next = match found.class {
        RecoveryClass::Resume | RecoveryClass::Retry => AgentStatus::Ready,
        RecoveryClass::Inspect => AgentStatus::Blocked {
            reason: BlockReason::Inspection {
                detail: found.detail.clone(),
            },
        },
    };
    if agent.status != next {
        agent.transition(next, now())?;
    }
    if let AgentStatus::Blocked { reason } = &agent.status {
        commit.push_event(recovery_event(
            agent,
            EventKind::AgentBlocked {
                reason: reason.describe(),
                dependency: reason.dependency(),
            },
        ));
        if !held.is_empty() {
            commit.push_event(recovery_event(
                agent,
                EventKind::ResourceAcquired {
                    keys: held.to_vec(),
                },
            ));
        }
    } else {
        commit.push_event(recovery_event(
            agent,
            EventKind::AgentResumed {
                reason: "recovered after restart".into(),
                via: None,
            },
        ));
    }
    if found.class == RecoveryClass::Retry {
        runtime.push_inbox(
            hold,
            commit,
            agent,
            aether_core::inbox::InboxKind::Wake {
                reason: found.detail.clone(),
            },
            Actor::System,
        )?;
    }
    Ok(())
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
        let held = match unreleased_keys(runtime, agent.id) {
            Ok(keys) => keys,
            Err(error) => {
                validation::quarantine_failed_recovery(runtime, agent.id, &error)?;
                continue;
            }
        };
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
        if last.seq <= after || page.len() > TURN_EVENT_PAGE {
            return Err(aether_store::StoreError::Corrupt(format!(
                "Agent {agent} recovery returned an invalid event page; run `aether doctor`"
            ))
            .into());
        }
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

/// Unreadable turn history cannot prove a retry safe. Resource history is
/// still read by the caller before any lease is cleared or agent is changed.
fn classify_or_inspect(
    runtime: &Runtime,
    agent: &aether_core::agent::Agent,
    rebooted: bool,
) -> Result<Interrupted> {
    match classify(runtime, agent, rebooted) {
        Ok(found) => Ok(found),
        Err(
            error @ crate::RuntimeError::Store(
                aether_store::StoreError::Serde(_)
                | aether_store::StoreError::Corrupt(_)
                | aether_store::StoreError::HistoryExpired { .. },
            ),
        ) => {
            let detail = format!(
                "Could not recover agent {} because its stored turn history is unreadable: \
                 {error}. Run `aether doctor` before attempting `aether resume {}`.",
                agent.id, agent.id
            );
            tracing::error!(agent = %agent.id, %error, "recovery requires inspection; run `aether doctor`");
            Ok(Interrupted {
                calls: Default::default(),
                class: RecoveryClass::Inspect,
                detail,
                open_calls: Vec::new(),
                cancelled_keys: Vec::new(),
                open_invocation: None,
            })
        }
        Err(error) => Err(error),
    }
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
