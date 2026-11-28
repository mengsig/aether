//! A Claude Code turn whose sign-in was refused relaunches a fresh CLI a few
//! times before it suspends: CLIs sharing one login race on its refresh.
use std::time::Duration;

use aether_core::{agent::Agent, model::ModelRef};

use super::{
    CancellationToken, Commit, EngineKind, EventKind, MAX_ENGINE_ERROR, ModelErrorClass, Result,
    Runtime, TurnCtx, clamp, json, now,
};

/// Recorded before every wait, so clients can show what the agent waits on.
pub(super) const AUTH_RETRY_EVENT: &str = "engine.auth_retry";

/// Each wait as a multiple of the floor: about three minutes at 10 s.
const STEPS: [u32; 4] = [1, 3, 6, 8];

/// ±30%, so children refused together do not relaunch together.
const JITTER: std::ops::RangeInclusive<f64> = 0.7..=1.3;

/// The relaunches one turn has left after refused sign-ins.
pub(super) struct AuthRetry {
    limit: u32,
    floor: Duration,
    waits: u32,
}

impl AuthRetry {
    pub(super) fn new(limit: u32, floor: Duration) -> Self {
        Self {
            limit,
            floor,
            waits: 0,
        }
    }

    /// Whether this failure is one a fresh launch may fix. Once the provider
    /// is known to refuse the sign-in - a turn on it gave up, or an agent is
    /// still suspended on it from before a restart - or this turn is itself a
    /// probe of a suspension, it suspends at once and the canary finds out
    /// when the sign-in works again.
    pub(super) fn applies(
        &self,
        runtime: &Runtime,
        agent: &Agent,
        engine: EngineKind,
        link: &ModelRef,
        class: ModelErrorClass,
    ) -> Result<bool> {
        if class != ModelErrorClass::Auth || engine != EngineKind::CLAUDE_CODE {
            return Ok(false);
        }
        if runtime.provider_auth(&link.provider)?.is_some() {
            return Ok(false);
        }
        Ok(!crate::exhaustion::episode_open(runtime, agent, link)?)
    }

    /// The next jittered wait, or `None` once the relaunches are spent.
    pub(super) fn next_delay(&mut self) -> Option<Duration> {
        self.next_delay_with(rand::random_range(JITTER))
    }

    fn next_delay_with(&mut self, spread: f64) -> Option<Duration> {
        debug_assert!(JITTER.contains(&spread), "spread {spread} out of range");
        if self.waits >= self.limit {
            return None;
        }
        let step = STEPS[(self.waits as usize).min(STEPS.len() - 1)];
        self.waits += 1;
        let delay = self.floor.saturating_mul(step).mul_f64(spread);
        debug_assert!(delay <= self.floor.saturating_mul(STEPS[3]).mul_f64(*JITTER.end()));
        Some(delay)
    }

    /// The terminal error's account of a wait another turn's refusal ended.
    pub(super) fn refused_elsewhere(&self) -> String {
        debug_assert!(self.waits > 0, "only a wait is ended by another turn");
        format!(
            "the provider refused the sign-in to this turn ({} launches) and to another",
            self.waits
        )
    }

    /// The terminal error's account of the relaunches, once they ran out.
    pub(super) fn gave_up(&self) -> String {
        format!(
            "the provider refused the sign-in on {} launches",
            self.waits + 1
        )
    }
}

/// How a wait before a relaunch ended.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Waited {
    Relaunch,
    Cancelled,
    /// Another turn on the provider gave up and suspended meanwhile.
    Refused,
}

/// Announce the wait, then sleep it unless the turn is cancelled or another
/// turn marks the provider refused first.
pub(super) async fn wait(
    runtime: &Runtime,
    ctx: &TurnCtx,
    retry: &AuthRetry,
    provider: &str,
    error: &str,
    delay: Duration,
    cancel: &CancellationToken,
) -> Result<Waited> {
    debug_assert!(retry.waits > 0, "a wait follows a counted failure");
    let next_at =
        now() + chrono::Duration::from_std(delay).unwrap_or_else(|_| chrono::Duration::zero());
    runtime.apply(Commit::new().event(ctx.draft(EventKind::Custom {
        name: AUTH_RETRY_EVENT.into(),
        payload: json!({
            "engine": EngineKind::CLAUDE_CODE.as_str(),
            "kind": aether_core::agent::PROVIDER_AUTH,
            "provider": provider,
            "attempt": retry.waits,
            "attempts": retry.limit,
            "delay_ms": u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            "next_at": next_at,
            "error": clamp(error, MAX_ENGINE_ERROR),
        }),
    })))?;
    tracing::warn!(agent = %ctx.agent.id, attempt = retry.waits, ?delay,
        "sign-in refused; relaunching the engine after a wait");
    let sleep = tokio::time::sleep(delay);
    tokio::pin!(sleep);
    loop {
        // Armed before the mark is read, so a mark set in between still wakes it.
        let marked = runtime.auth_refused.notified();
        tokio::pin!(marked);
        marked.as_mut().enable();
        if crate::scheduler::auth_marked(runtime, provider) {
            return Ok(Waited::Refused);
        }
        tokio::select! {
            () = cancel.cancelled() => return Ok(Waited::Cancelled),
            () = &mut sleep => break,
            () = &mut marked => {}
        }
    }
    if crate::scheduler::auth_marked(runtime, provider) {
        return Ok(Waited::Refused);
    }
    Ok(Waited::Relaunch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_relaunches_span_about_three_minutes_and_then_stop() {
        let mut low = AuthRetry::new(4, Duration::from_secs(10));
        let mut high = AuthRetry::new(4, Duration::from_secs(10));
        let lows: Vec<_> = (0..4).map(|_| low.next_delay_with(0.7).unwrap()).collect();
        let highs: Vec<_> = (0..4).map(|_| high.next_delay_with(1.3).unwrap()).collect();
        assert_eq!(low.next_delay_with(1.0), None, "the relaunches are spent");
        assert_eq!(low.waits, 4);
        let total = |delays: &[Duration]| delays.iter().sum::<Duration>().as_secs_f64();
        assert!((125.0..=127.0).contains(&total(&lows)), "{lows:?}");
        assert!((233.0..=235.0).contains(&total(&highs)), "{highs:?}");
        assert!(lows.windows(2).all(|pair| pair[0] < pair[1]), "{lows:?}");
        assert_eq!(
            low.gave_up(),
            "the provider refused the sign-in on 5 launches"
        );
    }

    #[test]
    fn jittered_waits_stay_inside_their_bounds() {
        for _ in 0..200 {
            let mut retry = AuthRetry::new(4, Duration::from_secs(10));
            for step in STEPS {
                let delay = retry.next_delay().unwrap();
                let base = Duration::from_secs(10 * u64::from(step));
                assert!(
                    delay >= base.mul_f64(0.7) && delay <= base.mul_f64(1.3),
                    "{delay:?}"
                );
            }
        }
        assert_eq!(
            AuthRetry::new(0, Duration::from_secs(10)).next_delay(),
            None
        );
    }
}
