use crate::{Result, Runtime};
use crate::{RuntimeError, SchedulerState};
use aether_core::{
    agent::{Agent, AgentStatus, BlockReason},
    event::{Actor, EventDraft, EventKind},
    id::AgentId,
    time::now,
};
use aether_core::{
    model::{ModelErrorClass, ModelRef},
    provider::FallbackRecord,
};
use aether_store::{AgentFilter, Commit, Mutation};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

/// Drop expired backoffs and say so, so a projection can show recovery.
pub(super) fn expire_backoff(runtime: &Runtime) -> Result<()> {
    let now_i = Instant::now();
    let recovered: Vec<String> = {
        let mut sched = runtime.sched.lock();
        let expired: Vec<String> = sched
            .backoff
            .iter()
            .filter(|(_, b)| b.until <= now_i)
            .map(|(name, _)| name.clone())
            .collect();
        for name in &expired {
            sched.backoff.remove(name);
        }
        expired
    };
    for provider in recovered {
        tracing::info!(%provider, "provider backoff lifted");
        runtime.apply(Commit::new().event(EventDraft::new(
            Actor::Scheduler,
            EventKind::ProviderRecovered { provider },
        )))?;
    }
    Ok(())
}

/// The one suspended agent per provider whose probe is due and is due first.
/// One probe per provider: a limit that spent one agent spent every agent on
/// it, so releasing them together would be a thundering herd against a
/// provider that has already said no.
pub(super) fn due_canaries(
    runtime: &Runtime,
    live: &[Agent],
    at: aether_core::time::Timestamp,
) -> HashMap<String, AgentId> {
    let in_flight = prune_probe_claims(runtime, live);
    let mut best: HashMap<String, (aether_core::time::Timestamp, AgentId)> = HashMap::new();
    for agent in live {
        // A paused agent receives no turns, so it never probes either.
        if agent.paused {
            continue;
        }
        let AgentStatus::Blocked {
            reason: BlockReason::Exhausted {
                model, next_probe, ..
            },
        } = &agent.status
        else {
            continue;
        };
        if *next_probe > at || in_flight.contains(&model.provider) {
            continue;
        }
        // Ties break on the agent id, which is time-ordered: the agent that
        // has been waiting longest probes first, and the pick is deterministic.
        let candidate = (*next_probe, agent.id);
        match best.get(&model.provider) {
            Some(held) if *held <= candidate => {}
            _ => {
                best.insert(model.provider.clone(), candidate);
            }
        }
    }
    best.into_iter().map(|(p, (_, id))| (p, id)).collect()
}

/// Providers with a probe in flight, after dropping claims whose canary can no
/// longer be carrying one. `RunningSlot` drops a claim when the probe turn
/// ends; this drops one whose agent never reached a turn at all - cancelled,
/// paused, or gone terminal - which would otherwise park the provider.
fn prune_probe_claims(runtime: &Runtime, live: &[Agent]) -> HashSet<String> {
    let dispatchable: HashSet<AgentId> = live
        .iter()
        // Paused is orthogonal to status: a released canary paused before it
        // got a slot never takes its turn, so it must not hold the provider.
        .filter(|a| !a.paused && matches!(a.status, AgentStatus::Ready | AgentStatus::Running))
        .map(|a| a.id)
        .collect();
    let mut sched = runtime.sched.lock();
    let state = &mut *sched;
    state
        .probing
        .retain(|_, id| state.running.contains_key(id) || dispatchable.contains(id));
    state
        .probing
        .keys()
        .cloned()
        .chain(
            state
                .usage_refresh
                .iter()
                .filter(|(_, refresh)| refresh.in_flight)
                .map(|(id, _)| id.clone()),
        )
        .collect()
}

/// Drop the claim this agent held, wherever it held one. Called when its probe
/// gets through and again when its turn ends, so no provider stays parked on an
/// agent that is no longer probing it.
pub(crate) fn release_probe_claim_of(state: &mut SchedulerState, agent: AgentId) {
    state.probing.retain(|_, id| *id != agent);
}

/// A person decided to probe now, so the claim another agent held on this
/// provider is no longer the scheduler's to keep.
pub(crate) fn clear_probe_claim(runtime: &Runtime, provider: &str) {
    runtime.sched.lock().probing.remove(provider);
}

/// The probe answered: every other agent parked on this provider may try now.
/// The tick still releases them one at a time, so a provider that only just
/// came back is not hit by the whole fleet at once. `hold` is the proof the
/// commit lock is held - these rows are read here and written by the same
/// commit, so nothing may move them in between.
pub(crate) fn provider_recovered_by_probe(
    runtime: &Runtime,
    _hold: &crate::CommitHold<'_>,
    commit: &mut Commit,
    provider: &str,
    probe: AgentId,
) -> Result<()> {
    let at = now();
    for mut agent in runtime.store.agents(&AgentFilter {
        live_only: true,
        ..Default::default()
    })? {
        if agent.id == probe {
            continue;
        }
        let AgentStatus::Blocked {
            reason: BlockReason::Exhausted {
                model, next_probe, ..
            },
        } = &mut agent.status
        else {
            continue;
        };
        if model.provider != provider || *next_probe <= at {
            continue;
        }
        *next_probe = at;
        agent.updated_at = at;
        commit.push_mutation(Mutation::UpsertAgent(agent));
    }
    Ok(())
}

/// Remember a normalized failure against the provider that returned it, so
/// `aether providers` can say what a provider has been failing with rather
/// than only that it is backed off.
pub(crate) fn record_error(runtime: &Runtime, provider: &str, class: ModelErrorClass) {
    runtime
        .sched
        .lock()
        .errors
        .entry(provider.to_owned())
        .or_default()
        .record(class, now());
}

/// The provider refused a sign-in and a turn suspended on it. The first
/// refusal of an episode keeps its time; later ones do not move it.
/// Turns waiting to relaunch on the provider are woken to suspend too.
pub(crate) fn mark_auth_failed(runtime: &Runtime, provider: &str) {
    debug_assert!(!provider.is_empty(), "a refused sign-in names its provider");
    let at = now();
    let mut sched = runtime.sched.lock();
    sched.auth_failed.entry(provider.to_owned()).or_insert(at);
    let last = sched
        .auth_refused_last
        .entry(provider.to_owned())
        .or_insert(at);
    *last = (*last).max(at);
    drop(sched);
    runtime.auth_refused.notify_waiters();
}

/// Whether a turn on `provider` suspended on a refused sign-in since its last answer.
pub(crate) fn auth_marked(runtime: &Runtime, provider: &str) -> bool {
    runtime.sched.lock().auth_failed.contains_key(provider)
}

/// The durable record that a turn on `provider` answered while its sign-in
/// was known refused: suspended rows older than it say nothing any more.
pub(crate) fn auth_answered_event(provider: &str) -> String {
    debug_assert!(!provider.is_empty(), "an answer names its provider");
    format!("provider.auth_answered.{provider}")
}

/// Whether the bench on `provider` is the one a refused sign-in put up.
pub(crate) fn credentials_benched(runtime: &Runtime, provider: &str) -> bool {
    runtime
        .sched
        .lock()
        .backoff
        .get(provider)
        .is_some_and(|bench| bench.reason == credentials_bench_reason())
}

fn credentials_bench_reason() -> String {
    format!(
        "{} from the provider",
        aether_core::agent::ExhaustionClass::Credentials.as_str()
    )
}

/// After the commit recording an answer at `at` on a refused `provider`
/// landed: the answer fences older rows, and the mark and the credentials
/// bench go unless the provider refused again after `at`.
pub(crate) fn settle_auth_answer(
    runtime: &Runtime,
    provider: &str,
    at: aether_core::time::Timestamp,
) {
    debug_assert!(!provider.is_empty(), "an answer names its provider");
    let mut sched = runtime.sched.lock();
    let fence = sched
        .auth_answered
        .entry(provider.to_owned())
        .or_insert(None);
    *fence = Some(fence.map_or(at, |earlier| earlier.max(at)));
    let refused_since = sched
        .auth_refused_last
        .get(provider)
        .is_some_and(|last| *last > at);
    if refused_since {
        drop(sched);
        tracing::info!(%provider, "a sign-in answered, but the provider refused one since");
        return;
    }
    sched.auth_failed.remove(provider);
    sched.auth_refused_last.remove(provider);
    let benched = sched
        .backoff
        .get(provider)
        .is_some_and(|bench| bench.reason == credentials_bench_reason());
    if benched {
        sched.backoff.remove(provider);
    }
    debug_assert!(!sched.auth_failed.contains_key(provider));
    drop(sched);
    tracing::info!(%provider, benched, "provider sign-in answered again");
}

/// Remember where a turn went after giving up on `from`, so a projection can
/// show the chain move without replaying the event log.
pub(crate) fn record_fallback(
    runtime: &Runtime,
    from: &str,
    to: &ModelRef,
    reason: &str,
    class: Option<ModelErrorClass>,
) {
    runtime.sched.lock().fallbacks.insert(
        from.to_owned(),
        FallbackRecord {
            to: to.clone(),
            reason: reason.to_owned(),
            class,
            at: now(),
        },
    );
}

/// Ask the scheduler to avoid a provider for a while. Extending an existing
/// backoff keeps the later expiry and records the newer reason.
pub(crate) fn backoff(
    runtime: &Runtime,
    provider: &str,
    reason: String,
    for_: Duration,
) -> Result<()> {
    let until = Instant::now() + for_;
    let until_at =
        now() + chrono::Duration::from_std(for_).unwrap_or_else(|_| chrono::Duration::zero());
    let announce = {
        let mut sched = runtime.sched.lock();
        match sched.backoff.get_mut(provider) {
            Some(existing) if existing.until >= until => false,
            Some(existing) => {
                existing.until = until;
                existing.until_at = until_at;
                existing.reason = reason.clone();
                true
            }
            None => {
                sched.backoff.insert(
                    provider.to_owned(),
                    crate::Backoff {
                        until,
                        until_at,
                        reason: reason.clone(),
                    },
                );
                true
            }
        }
    };
    if !announce {
        return Ok(());
    }
    tracing::info!(%provider, %reason, "backing off a provider");
    // The log is the only projection a person has: holding turns back with
    // nothing to explain why is worse than failing the caller.
    runtime.apply(Commit::new().event(EventDraft::new(
        Actor::Scheduler,
        EventKind::ProviderBackoff {
            provider: provider.to_owned(),
            reason,
            until: until_at,
        },
    )))?;
    Ok(())
}

/// Lift a provider's bench because a person decided to try it now.
pub(crate) fn clear_bench(runtime: &Runtime, provider: &str) -> Result<()> {
    if runtime.sched.lock().backoff.remove(provider).is_none() {
        return Ok(());
    }
    tracing::info!(%provider, "provider backoff dropped by an operator resume");
    runtime.apply(Commit::new().event(EventDraft::new(
        Actor::Scheduler,
        EventKind::ProviderRecovered {
            provider: provider.to_owned(),
        },
    )))?;
    Ok(())
}

/// A subscription window near its limit is provider feedback like a 429:
/// record it, and hold turns back while it stays that high.
const QUOTA_BACKOFF_PERCENT: u64 = 95;
/// Long enough to stop hammering a spent window, short enough that one turn
/// re-reads the provider's own report soon after.
pub(crate) const QUOTA_BACKOFF: Duration = Duration::from_secs(60);

/// Record a subscription window a provider reported.
pub(crate) fn record_quota(
    runtime: &Runtime,
    provider: &str,
    usage: aether_core::provider::ProviderUsage,
) -> Result<()> {
    let window = usage
        .preferred_window()
        .ok_or_else(|| RuntimeError::Invalid("provider usage has no windows".into()))?;
    runtime.apply(Commit::new().event(EventDraft::new(
        Actor::Scheduler,
        EventKind::ProviderQuota {
            provider: provider.to_owned(),
            window,
            usage: Some(usage.clone()),
        },
    )))?;
    {
        let mut scheduler = runtime.sched.lock();
        if scheduler
            .quotas
            .get(provider)
            .is_some_and(|latest| latest.observed_at > usage.observed_at)
        {
            return Ok(());
        }
        scheduler.quotas.insert(provider.to_owned(), usage);
    }
    if window.used_percent >= QUOTA_BACKOFF_PERCENT {
        backoff(
            runtime,
            provider,
            format!("subscription window {}% spent", window.used_percent),
            QUOTA_BACKOFF,
        )?;
    }
    Ok(())
}

/// Reserve before waking, under the same lock used by usage refresh admission.
pub(super) fn claim_canary(runtime: &Runtime, provider: &str, agent: AgentId) -> bool {
    let mut sched = runtime.sched.lock();
    if sched.probing.contains_key(provider)
        || sched
            .usage_refresh
            .get(provider)
            .is_some_and(|refresh| refresh.in_flight)
    {
        return false;
    }
    debug_assert!(!provider.is_empty());
    let previous = sched.probing.insert(provider.to_owned(), agent);
    debug_assert!(previous.is_none());
    true
}
