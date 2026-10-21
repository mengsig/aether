use super::{Runnable, lineage::Lineage};
use crate::{RunningTurn, Runtime, SchedulerState};
use aether_core::{agent::Agent, id::AgentId, model::ModelRef};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

/// Start turns for as many ready agents as the limits of VISION §3.6 allow,
/// in that order. A person's message bypasses limits 2 to 4; it never
/// bypasses limit 5, because two agents mutating the same file is data loss,
/// not a queueing decision (see ARCHITECTURE, resource leases).
pub(super) fn dispatch(runtime: &Arc<Runtime>, ready: Vec<Runnable>, lineage: &Lineage) {
    let limits = provider_limits(runtime, &ready);
    let engines = engine_providers(runtime, &ready);
    let mut sched = runtime.sched.lock();
    // A stopped daemon starts no turn. Read under the lock `shutdown` takes,
    // so a turn dispatched just before the stop is still one it waits for.
    if runtime.shutdown.is_cancelled() {
        return;
    }
    let mut remaining = ready;
    track_ready_since(&mut sched, &remaining);
    // Greedy: each pick sees the turns already started this pass, so equal
    // candidates spread across providers instead of piling on one.
    while let Some(index) = pick(runtime, &remaining, &sched, lineage, &limits, &engines) {
        // Not swap_remove: `pick`'s index tiebreak is the ready order.
        let Runnable { agent, addressed } = remaining.remove(index);
        let cancel = CancellationToken::new();
        let interrupt = cancel.child_token();
        // The turn starts on the first link that can carry it; the turn keeps
        // this up to date as it walks the rest of the chain.
        let provider = available_link(&agent, &sched, &limits)
            .unwrap_or(&agent.model)
            .provider
            .clone();
        sched.running.insert(
            agent.id,
            RunningTurn {
                task: None,
                cancel: cancel.clone(),
                cancel_request: None,
                stop_reason: None,
                interrupt: interrupt.clone(),
                addressed,
                engine: engines.contains(&provider),
                provider,
                on_provider: true,
            },
        );
        let rt = runtime.clone();
        let id = agent.id;
        tokio::spawn(async move {
            crate::turn::run(rt, id, cancel, interrupt).await;
        });
    }
}

/// Configured concurrency cap per provider any candidate could land on -
/// every link of every candidate's chain, not just its primary.
fn provider_limits(runtime: &Runtime, ready: &[Runnable]) -> HashMap<String, u32> {
    let registry = runtime.models.registry();
    ready
        .iter()
        .flat_map(|r| r.agent.chain().links().cloned().collect::<Vec<_>>())
        .filter_map(|link| {
            let limit = registry
                .provider(&link.provider)
                .and_then(|p| p.max_concurrent)?;
            Some((link.provider, limit))
        })
        .collect()
}

/// Which of the candidates' providers are engines (Claude Code, Codex), so a
/// dispatch pass classifies a link by set lookup instead of walking the
/// registry once per pick.
fn engine_providers(runtime: &Runtime, ready: &[Runnable]) -> HashSet<String> {
    let registry = runtime.models.registry();
    ready
        .iter()
        .flat_map(|r| r.agent.chain().links().cloned().collect::<Vec<_>>())
        .filter(|link| {
            registry
                .provider(&link.provider)
                .is_some_and(|p| p.protocol.is_engine())
        })
        .map(|link| link.provider)
        .collect()
}

/// Why a link cannot carry a turn right now, if it cannot: its provider is
/// backed off, or the provider's configured concurrency cap is already spent.
///
/// The one rule admission and execution share, so a turn runs on the link it
/// was admitted for. `asking` is the agent enquiring about its own turn: that
/// turn already holds a slot on some link, so it is left out of the count and
/// never blocks itself.
fn link_unavailable_in(
    sched: &SchedulerState,
    provider: &str,
    cap: Option<u32>,
    asking: Option<AgentId>,
) -> Option<String> {
    if let Some(backoff) = sched.backoff.get(provider) {
        return Some(backoff.reason.clone());
    }
    let cap = cap? as usize;
    let running = sched
        .on_provider()
        .filter(|(id, r)| r.provider == provider && Some(**id) != asking)
        .count();
    (running >= cap).then(|| format!("`{provider}` is at its limit of {cap} concurrent turns"))
}

/// The first link of an agent's chain that is neither backed off nor at its
/// configured cap. `None` when every link is held back, which is what makes
/// the agent ineligible rather than one unlucky provider.
fn available_link<'a>(
    agent: &'a Agent,
    sched: &SchedulerState,
    limits: &HashMap<String, u32>,
) -> Option<&'a ModelRef> {
    std::iter::once(&agent.model)
        .chain(agent.fallbacks.iter())
        .find(|link| {
            link_unavailable_in(
                sched,
                &link.provider,
                limits.get(&link.provider).copied(),
                None,
            )
            .is_none()
        })
}

/// Whether the link this agent would be admitted on is an engine - the same
/// link `dispatch` records on the turn, so the cap counts what actually runs
/// rather than the agent's primary model.
fn runs_on_engine(
    agent: &Agent,
    sched: &SchedulerState,
    limits: &HashMap<String, u32>,
    engines: &HashSet<String>,
) -> bool {
    let link = available_link(agent, sched, limits).unwrap_or(&agent.model);
    engines.contains(&link.provider)
}

/// How long an agent whose turn could enter no link waits before it is
/// offered another. One scheduler period: long enough that a notify-driven
/// re-tick cannot spin on it, short enough to be invisible when the slot or
/// the backoff frees.
const ENTRY_REFUSAL_HOLD: Duration = Duration::from_secs(1);

/// Damp a turn handed back because no link would carry it. A person's message
/// still bypasses this, like every other limit below limit 5.
pub(crate) fn hold_after_entry_refusal(runtime: &Runtime, agent: AgentId) {
    runtime
        .sched
        .lock()
        .entry_refused_until
        .insert(agent, Instant::now() + ENTRY_REFUSAL_HOLD);
}

/// When this candidate became eligible, for limit 6's ordering. The stamp is
/// the agent's `updated_at` as first seen ready, so it orders by arrival and
/// then stops moving while the candidate waits.
fn ready_since(sched: &SchedulerState, agent: &Agent) -> aether_core::time::Timestamp {
    sched
        .ready_since
        .get(&agent.id)
        .copied()
        .unwrap_or(agent.updated_at)
}

/// Stamp candidates newly seen as ready and drop everyone no longer waiting,
/// so `ready_since` measures this queue rather than growing without bound.
fn track_ready_since(sched: &mut SchedulerState, ready: &[Runnable]) {
    for r in ready {
        sched
            .ready_since
            .entry(r.agent.id)
            .or_insert(r.agent.updated_at);
    }
    if sched.ready_since.len() > ready.len() {
        let still_waiting: HashSet<AgentId> = ready.iter().map(|r| r.agent.id).collect();
        sched.ready_since.retain(|id, _| still_waiting.contains(id));
    }
    let now = Instant::now();
    sched.entry_refused_until.retain(|_, until| *until > now);
}

/// The next agent to run, or `None` when no candidate passes the limits.
/// Limits 1-4 decide eligibility; limit 6 orders what is left.
pub(super) fn pick(
    runtime: &Runtime,
    ready: &[Runnable],
    sched: &SchedulerState,
    lineage: &Lineage,
    limits: &HashMap<String, u32>,
    engines: &HashSet<String>,
) -> Option<usize> {
    // A turn parked on a resource key is making no model call: it holds no
    // global, subtree or provider slot while it waits.
    let at = Instant::now();
    let global_full = sched.on_provider().count() >= runtime.config.max_concurrent_turns;
    // A resuming turn queued for a slot counts here: it is holding resource
    // keys while it waits, so handing its slot to a newcomer stalls everyone
    // behind those keys too.
    let engine_full =
        !sched.engine_slot_free_for_newcomer(runtime.config.max_concurrent_engine_turns);
    let running_on = |provider: &str| {
        sched
            .on_provider()
            .filter(|(_, r)| r.provider == provider)
            .count()
    };
    // Under the engine cap every engine candidate competes for one pool, so
    // "how busy is my provider" is the pool's occupancy, not the link's.
    // Otherwise a fresh arrival on an idle engine outranks a candidate that
    // has waited arbitrarily long on a busy one - starvation, not a tiebreak.
    let pooled = runtime
        .config
        .max_concurrent_engine_turns
        .map(|_| sched.on_engine());
    let busy = |provider: &str| match pooled {
        Some(in_pool) if engines.contains(provider) => in_pool,
        _ => running_on(provider),
    };
    ready
        .iter()
        .enumerate()
        .filter(|(_, r)| !sched.running.contains_key(&r.agent.id))
        .filter(|(_, r)| !lineage.has_unreadable_ancestor(r.agent.id))
        // 0. A turn just handed back because no link would carry it: give the
        //    condition a moment to clear instead of re-dispatching this tick.
        .filter(|(_, r)| {
            r.addressed
                || sched
                    .entry_refused_until
                    .get(&r.agent.id)
                    .is_none_or(|until| *until <= at)
        })
        // 1. People first: past every limit below.
        // 2. Global limit.
        .filter(|(_, r)| r.addressed || !global_full)
        // 3. Each supervisor's cap on its subtree.
        .filter(|(_, r)| r.addressed || !lineage.capped(r.agent.id, sched))
        // 4. Per-provider limits, configured and from provider feedback. A
        //    fallback chain passes while any link can still carry the turn.
        .filter(|(_, r)| r.addressed || available_link(&r.agent, sched, limits).is_some())
        // 4b. The daemon's engine cap, within the global limit: engine turns
        //     are what a subscription window is spent on, and 13 of them at
        //     once burn 13x a single agent's rate. An API turn
        //     passes untouched. Only a turn actually running on an engine
        //     holds a slot, so a parent waiting on its children holds none
        //     and the cap cannot deadlock a subtree.
        .filter(|(_, r)| {
            r.addressed || !engine_full || !runs_on_engine(&r.agent, sched, limits, engines)
        })
        // 6. Priority, then the least busy provider so healthy ones share the
        //    global slots, then longest waiting. Wait time is "ready since",
        //    not `updated_at`: an inbox delivery or a status write must not
        //    send a candidate to the back of the queue.
        .min_by_key(|(index, r)| {
            let link = available_link(&r.agent, sched, limits).unwrap_or(&r.agent.model);
            (
                !r.addressed,
                std::cmp::Reverse(r.agent.config.priority),
                busy(&link.provider),
                ready_since(sched, &r.agent),
                *index,
            )
        })
        .map(|(index, _)| index)
}

/// Move a running turn onto `provider`, or say why that link cannot carry it.
///
/// The turn asks this for every link it walks, by the same rule `pick`
/// admitted it under, so a cap the scheduler honoured is not exceeded
/// downstream. Check and claim happen under one lock - like `dispatch` - so
/// two turns cannot both take the last slot on a capped provider.
pub(crate) fn claim_link(
    runtime: &Runtime,
    agent: AgentId,
    provider: &str,
) -> std::result::Result<(), String> {
    let (cap, engine) = {
        let registry = runtime.models.registry();
        let def = registry.provider(provider);
        (
            def.and_then(|p| p.max_concurrent),
            def.is_some_and(|p| p.protocol.is_engine()),
        )
    };
    let freed_engine = {
        let mut sched = runtime.sched.lock();
        // Admission already reserved this link for the operator's reply.
        // Refusing it here leaves the same inbox item unread and immediately
        // eligible again, producing empty turns until the backoff expires.
        // A different fallback link or a released slot still needs admission.
        let addressed = sched
            .running
            .get(&agent)
            .is_some_and(|turn| turn.addressed && turn.on_provider && turn.provider == provider);
        if !addressed && let Some(reason) = link_unavailable_in(&sched, provider, cap, Some(agent))
        {
            return Err(reason);
        }
        // A turn falling back from an API model onto an engine takes an engine
        // slot it was not admitted for; without this the cap would leak. Only
        // a turn that is *holding* a slot skips the check: a parked one gave
        // its slot away and has to queue for another like anyone else.
        let already_on_engine = sched
            .running
            .get(&agent)
            .is_some_and(|r| r.on_provider && r.engine);
        if engine
            && !addressed
            && !already_on_engine
            && let Some(cap) = runtime.config.max_concurrent_engine_turns
            && !sched.engine_slot_free_for_newcomer(Some(cap))
        {
            return Err(format!(
                "the daemon is at its limit of {cap} concurrent engine turns"
            ));
        }
        // Follow the turn to the provider actually carrying it, so the
        // per-provider limit counts the link in flight rather than the primary.
        match sched.running.get_mut(&agent) {
            Some(running) => {
                let freed = running.on_provider && running.engine && !engine;
                provider.clone_into(&mut running.provider);
                running.engine = engine;
                freed
            }
            None => false,
        }
    };
    // Falling back off an engine onto an API link gives the slot back.
    if freed_engine {
        runtime.engine_slot.notify_waiters();
    }
    Ok(())
}
