use crate::{Result, Runtime, SchedulerState};
use aether_core::{
    provider::{ProviderBackoff, ProviderHealth},
    time::{Timestamp, now},
};
use std::collections::{BTreeSet, HashMap};

impl Runtime {
    /// Derive freshness at read time without probing or changing scheduling state.
    pub fn provider_headroom(
        &self,
        usage: Option<&aether_core::provider::ProviderUsage>,
        at: Timestamp,
    ) -> aether_core::provider::ProviderHeadroom {
        aether_core::provider::ProviderHeadroom::from_usage(
            usage,
            at,
            std::time::Duration::from_secs(self.models.config().providers.usage_stale_after),
        )
    }

    /// All enabled providers, including subscriptions with no observation yet.
    pub fn provider_usage(&self) -> serde_json::Value {
        let at = now();
        let sched = self.sched.lock();
        let providers: Vec<_> = self
            .models
            .registry()
            .providers()
            .iter()
            .filter(|provider| provider.enabled)
            .map(|provider| {
                serde_json::json!({
                    "provider": provider.id,
                    "headroom": self.provider_headroom(sched.quotas.get(&provider.id), at),
                })
            })
            .collect();
        serde_json::json!({"providers": providers})
    }

    /// What each provider is carrying, what it may carry, whether the
    /// scheduler is holding it back, what it has been failing with and where
    /// a turn last went instead. Providers with nothing in flight and no
    /// feedback inside `ErrorWindow::WINDOW` are omitted, so the list shrinks
    /// again as failures age out instead of growing for the daemon's life.
    pub fn provider_health(&self) -> Result<Vec<ProviderHealth>> {
        let at = now();
        // Read before the lock: the store is I/O and the scheduler waits on it.
        let suspended = self.suspended_agents()?;
        let sched = self.sched.lock();
        let mut names: BTreeSet<&str> = BTreeSet::new();
        names.extend(sched.running.values().map(|r| r.provider.as_str()));
        names.extend(sched.backoff.keys().map(String::as_str));
        names.extend(sched.quotas.keys().map(String::as_str));
        names.extend(sched.errors.keys().map(String::as_str));
        names.extend(sched.fallbacks.keys().map(String::as_str));
        names.extend(suspended.keys().map(String::as_str));
        Ok(names
            .into_iter()
            .map(|provider| self.health(&sched, &suspended, provider, at))
            .filter(|health| health.has_live_feedback(at))
            .collect())
    }

    /// One provider's picture, whether or not it has done anything yet, so a
    /// listing over the registry has a row for every provider.
    pub fn provider_health_for(&self, provider: &str) -> Result<ProviderHealth> {
        let at = now();
        let suspended = self.suspended_agents()?;
        let sched = self.sched.lock();
        Ok(self.health(&sched, &suspended, provider, at))
    }

    /// Agents parked on each provider's exhaustion, counted from the rows:
    /// the row is the only source of truth for a suspension, and it outlives
    /// any in-memory tally across a restart.
    fn suspended_agents(&self) -> Result<HashMap<String, u32>> {
        let mut counts: HashMap<String, u32> = HashMap::new();
        // Filtered in SQL: `aether providers` asks per provider and the TUI
        // asks every couple of seconds, and blocked rows are usually none.
        for agent in self.store.agents(&aether_store::AgentFilter {
            live_only: true,
            statuses: vec![aether_core::agent::AgentStatusKind::Blocked],
            ..Default::default()
        })? {
            if let aether_core::agent::AgentStatus::Blocked {
                reason: aether_core::agent::BlockReason::Exhausted { model, .. },
            } = &agent.status
            {
                *counts.entry(model.provider.clone()).or_default() += 1;
            }
        }
        Ok(counts)
    }

    /// `at` is the instant the whole listing is read at, so every row ages its
    /// samples against the same clock.
    fn health(
        &self,
        sched: &SchedulerState,
        suspended: &HashMap<String, u32>,
        provider: &str,
        at: Timestamp,
    ) -> ProviderHealth {
        ProviderHealth {
            running: sched
                .running
                .values()
                .filter(|r| r.on_provider && r.provider == provider)
                .count() as u32,
            suspended_agents: suspended.get(provider).copied().unwrap_or(0),
            limit: self
                .models
                .registry()
                .provider(provider)
                .and_then(|p| p.max_concurrent),
            backoff: sched.backoff.get(provider).map(|b| ProviderBackoff {
                reason: b.reason.clone(),
                until: b.until_at,
            }),
            quota: sched
                .quotas
                .get(provider)
                .and_then(|usage| usage.preferred_window()),
            usage: sched.quotas.get(provider).cloned(),
            recent_errors: sched
                .errors
                .get(provider)
                .map(|w| w.tallies(at))
                .unwrap_or_default(),
            last_fallback: sched.fallbacks.get(provider).cloned(),
            provider: provider.to_owned(),
        }
    }
}
