use crate::{Result, Runtime, RuntimeError};
use aether_core::{provider::ProviderUsage, time::now};
use aether_model::registry::ProviderDef;
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

mod probe;
#[cfg(test)]
mod tests;

#[derive(Debug)]
pub(crate) struct RefreshState {
    pub last_attempt: Instant,
    pub in_flight: bool,
    pub quarantined: bool,
}

struct Claim {
    runtime: Arc<Runtime>,
    provider: String,
}

impl Drop for Claim {
    fn drop(&mut self) {
        let mut sched = self.runtime.sched.lock();
        if let Some(state) = sched.usage_refresh.get_mut(&self.provider) {
            debug_assert!(state.in_flight);
            state.in_flight = false;
        }
    }
}

impl Runtime {
    /// Refresh stale enabled providers; independent failures remain in their rows.
    pub async fn refresh_provider_usage(self: &Arc<Self>, provider: Option<&str>) -> Result<Value> {
        let providers: Vec<_> = self
            .models
            .registry()
            .providers()
            .iter()
            .filter(|p| p.enabled && provider.is_none_or(|id| id == p.id))
            .cloned()
            .collect();
        if provider.is_some() && providers.is_empty() {
            return Err(RuntimeError::Invalid("unknown or disabled provider".into()));
        }
        let mut rows = Vec::with_capacity(providers.len());
        for provider in providers {
            let (status, error) = self.refresh_one(&provider).await;
            let sched = self.sched.lock();
            let mut row = json!({
                "provider": provider.id,
                "headroom": self.provider_headroom(sched.quotas.get(&provider.id), now()),
                "status": status,
            });
            if let Some(error) = error {
                row["error"] = json!(error);
            }
            rows.push(row);
        }
        Ok(json!({"providers": rows}))
    }

    fn claim_usage_refresh(
        self: &Arc<Self>,
        provider: &str,
    ) -> std::result::Result<Claim, &'static str> {
        let mut sched = self.sched.lock();
        let at = Instant::now();
        if !self
            .provider_headroom(sched.quotas.get(provider), now())
            .stale
        {
            return Err("cached");
        }
        if sched.backoff.get(provider).is_some_and(|b| b.until > at) {
            return Err("backoff");
        }
        if sched.probing.contains_key(provider) {
            return Err("in_flight");
        }
        let interval = Duration::from_secs(self.models.config().providers.usage_refresh_interval);
        if let Some(previous) = sched.usage_refresh.get(provider) {
            if previous.quarantined {
                return Err("quarantined");
            }
            if previous.in_flight {
                return Err("in_flight");
            }
            if at.duration_since(previous.last_attempt) < interval {
                return Err("cached");
            }
        }
        debug_assert!(!provider.is_empty());
        debug_assert!(!interval.is_zero());
        sched.usage_refresh.insert(
            provider.to_owned(),
            RefreshState {
                last_attempt: at,
                in_flight: true,
                quarantined: false,
            },
        );
        Ok(Claim {
            runtime: self.clone(),
            provider: provider.to_owned(),
        })
    }

    async fn refresh_one(
        self: &Arc<Self>,
        provider: &ProviderDef,
    ) -> (&'static str, Option<String>) {
        let claim = match self.claim_usage_refresh(&provider.id) {
            Ok(claim) => claim,
            Err(status) => {
                let error = (status == "quarantined").then(|| {
                    "usage refresh cleanup failed; further refreshes disabled until restart".into()
                });
                return (status, error);
            }
        };
        let cancel = self.shutdown.child_token();
        let _cancel_on_drop = cancel.clone().drop_guard();
        let provider = provider.clone();
        // The owner survives caller cancellation until the engine has cleaned up.
        let owner = tokio::spawn(async move { run_refresh(claim, provider, cancel).await });
        match owner.await {
            Ok(Ok(Some(()))) => ("refreshed", None),
            Ok(Ok(None)) => ("unsupported", None),
            Ok(Err(error)) => ("error", Some(error)),
            Err(error) => ("error", Some(format!("usage refresh task failed: {error}"))),
        }
    }
}

async fn run_refresh(
    claim: Claim,
    provider: ProviderDef,
    cancel: CancellationToken,
) -> std::result::Result<Option<()>, String> {
    let _cancel_on_drop = cancel.clone().drop_guard();
    let runtime = claim.runtime.clone();
    let timeout = Duration::from_secs(runtime.models.config().providers.usage_refresh_timeout);
    let work = probe::read(&runtime, &provider, cancel.clone());
    tokio::pin!(work);
    debug_assert!(runtime.sched.lock().usage_refresh[&provider.id].in_flight);
    debug_assert!(!timeout.is_zero());
    let failure = tokio::select! {
        result = &mut work => return record_refresh(&runtime, &provider.id, result, &cancel),
        _ = tokio::time::sleep(timeout) => "usage refresh timed out",
        _ = cancel.cancelled() => "usage refresh cancelled",
    };
    cancel.cancel();
    match tokio::time::timeout(Duration::from_secs(5), &mut work).await {
        Ok(Ok(_)) => Err(failure.into()),
        Ok(Err(error)) => Err(format!("{failure}; {error}")),
        Err(_) => {
            if let Some(state) = runtime.sched.lock().usage_refresh.get_mut(&provider.id) {
                debug_assert!(state.in_flight);
                state.quarantined = true;
            }
            Err(
                "usage refresh cleanup timed out; provider refresh quarantined until restart"
                    .into(),
            )
        }
    }
}

fn record_refresh(
    runtime: &Runtime,
    provider: &str,
    result: std::result::Result<Option<Value>, String>,
    cancel: &CancellationToken,
) -> std::result::Result<Option<()>, String> {
    let Some(payload) = result? else {
        return Ok(None);
    };
    if cancel.is_cancelled() {
        return Err("usage refresh cancelled".into());
    }
    let usage = ProviderUsage::from_engine_payload(&payload, now())
        .ok_or_else(|| "usage refresh returned no valid quota windows".to_owned())?;
    crate::scheduler::record_quota(runtime, provider, usage).map_err(|e| e.to_string())?;
    Ok(Some(()))
}
