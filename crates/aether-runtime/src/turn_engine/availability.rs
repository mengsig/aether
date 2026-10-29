//! A turn whose engine cannot be launched right now - its CLI caught
//! mid-update, a probe that failed under load - probes it again under the
//! transient backoff instead of failing an agent that already exists.
use std::sync::Arc;
use std::time::Duration;

use aether_engine::Engine;

use super::transient::{Backoff, TRANSIENT_RETRY_EVENT};
use super::{
    CancellationToken, Commit, EngineKind, EventKind, MAX_ENGINE_ERROR, Result, Runtime, TurnCtx,
    clamp, json, now,
};

pub(super) enum Availability {
    Ready(Arc<dyn Engine>),
    /// Still not launchable when the retry window ran out: the failure text.
    Unavailable(String),
    Cancelled,
}

/// The engine this turn launches, once a probe finds it launchable. Waits
/// only for an engine that was usable before, here or for this agent's
/// session; gives up after `[scheduler] engine_transient_retry_minutes`.
/// Cancelling, or the window ending, also ends a probe still running.
pub(super) async fn wait_for_engine(
    runtime: &Runtime,
    ctx: &TurnCtx,
    kind: EngineKind,
    provider: &str,
    cancel: &CancellationToken,
) -> Result<Availability> {
    assert!(!provider.is_empty(), "an engine runs for a named provider");
    let mut backoff = Backoff::new(
        runtime.config.engine_transient_retry_window,
        runtime.config.engine_transient_retry_floor,
    );
    // The window opens now, so even a first probe that hangs ends with it.
    backoff.failed();
    let mut last_reason = unfinished_probe(runtime, kind, provider);
    loop {
        // Dropping a probe is safe: detection kills its CLI, and nothing is
        // recorded for a probe that did not finish. One already done wins.
        let detected = tokio::select! {
            biased;
            () = cancel.cancelled() => return Ok(Availability::Cancelled),
            detected = runtime.engines.recheck(kind) => detected,
            () = window_end(&backoff) => {
                return Ok(Availability::Unavailable(gave_up(last_reason, &backoff)));
            }
        };
        if let Some(engine) = detected.launchable(&kind) {
            debug_assert_eq!(engine.kind(), kind, "the catalog is keyed by kind");
            return Ok(Availability::Ready(engine));
        }
        let reason = crate::engine_access::unavailable_text(kind, provider, detected.reason(&kind));
        // Never found and never run on: not installed, not briefly away.
        if !detected.was_usable(&kind) && ctx.agent.engine_session.is_none() {
            return Ok(Availability::Unavailable(reason));
        }
        backoff.failed();
        let Some(delay) = backoff.next_delay() else {
            return Ok(Availability::Unavailable(gave_up(reason, &backoff)));
        };
        announce(runtime, ctx, kind, &backoff, delay, &reason)?;
        tracing::warn!(agent = %ctx.agent.id, engine = kind.as_str(), ?delay, %reason,
            "engine not launchable; probing it again before failing the turn");
        tokio::select! {
            () = cancel.cancelled() => return Ok(Availability::Cancelled),
            () = tokio::time::sleep(delay) => {}
        }
        if backoff.spent() {
            return Ok(Availability::Unavailable(gave_up(reason, &backoff)));
        }
        last_reason = reason;
    }
}

async fn window_end(backoff: &Backoff) {
    debug_assert!(
        backoff.started(),
        "the window opened before the first probe"
    );
    let left = backoff.window().saturating_sub(backoff.elapsed());
    tokio::time::sleep(left).await;
}

/// Why the engine is unusable when no probe of it has finished in the window:
/// the detection already on record, else the probe itself.
fn unfinished_probe(runtime: &Runtime, kind: EngineKind, provider: &str) -> String {
    match runtime.engines.now() {
        Ok(detected) if detected.launchable(&kind).is_none() => {
            crate::engine_access::unavailable_text(kind, provider, detected.reason(&kind))
        }
        _ => format!("the {} CLI probe did not finish", kind.as_str()),
    }
}

fn gave_up(reason: String, backoff: &Backoff) -> String {
    let waited = backoff.elapsed().as_secs();
    match waited {
        0 => reason,
        _ => format!("{reason} (still unusable after probing it again for {waited}s)"),
    }
}

/// The same event a transient failure's wait records, so clients show both.
fn announce(
    runtime: &Runtime,
    ctx: &TurnCtx,
    kind: EngineKind,
    backoff: &Backoff,
    delay: Duration,
    reason: &str,
) -> Result<()> {
    debug_assert!(!delay.is_zero(), "a wait has a length");
    let next_at =
        now() + chrono::Duration::from_std(delay).unwrap_or_else(|_| chrono::Duration::zero());
    runtime.apply(Commit::new().event(ctx.draft(EventKind::Custom {
        name: TRANSIENT_RETRY_EVENT.into(),
        payload: json!({
            "engine": kind.as_str(),
            "class": "detection",
            "waiting_for": "engine CLI",
            "attempt": backoff.waits(),
            "delay_ms": u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            "next_at": next_at,
            "retrying_for_secs": backoff.elapsed().as_secs(),
            "window_secs": backoff.window().as_secs(),
            "error": clamp(reason, MAX_ENGINE_ERROR),
        }),
    })))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KIND: EngineKind = EngineKind::CLAUDE_CODE;

    struct Idle;

    #[async_trait::async_trait]
    impl Engine for Idle {
        fn kind(&self) -> EngineKind {
            KIND
        }

        fn adapter(&self) -> String {
            "idle/1".into()
        }

        async fn run(
            &self,
            _turn: aether_engine::EngineTurn,
            _cancel: CancellationToken,
        ) -> std::result::Result<aether_engine::EngineRun, aether_engine::EngineError> {
            Err(aether_engine::EngineError::Unsupported("idle".into()))
        }
    }

    async fn waiting(window: Duration) -> (Arc<Runtime>, tempfile::TempDir, TurnCtx) {
        let (runtime, dir) = crate::test_runtime(|config| {
            config.engine_transient_retry_window = window;
            config.engine_transient_retry_floor = Duration::from_millis(20);
        })
        .await;
        let ctx = super::super::tests::started_agent_ctx(&runtime).await;
        (runtime, dir, ctx)
    }

    fn announced(runtime: &Runtime, ctx: &TurnCtx) -> usize {
        runtime
            .store
            .events(
                0,
                10_000,
                &aether_store::EventFilter::own_events(ctx.agent.id),
            )
            .unwrap()
            .iter()
            .filter(|event| {
                matches!(&event.kind, EventKind::Custom { name, payload }
                    if name == TRANSIENT_RETRY_EVENT && payload["waiting_for"] == "engine CLI")
            })
            .count()
    }

    /// An agent that already ran on the engine, whose CLI went missing since.
    async fn waiting_with_session(window: Duration) -> (Arc<Runtime>, tempfile::TempDir, TurnCtx) {
        let (runtime, dir, mut ctx) = waiting(window).await;
        ctx.agent.engine_session = Some("kept".into());
        (runtime, dir, ctx)
    }

    #[tokio::test]
    async fn an_engine_that_comes_back_within_the_window_is_launched() {
        let (runtime, _dir, ctx) = waiting_with_session(Duration::from_secs(30)).await;
        let restoring = runtime.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            restoring.engines.mark_usable(KIND, Arc::new(Idle));
        });
        let cancel = CancellationToken::new();
        let found = wait_for_engine(&runtime, &ctx, KIND, "claude-code", &cancel)
            .await
            .unwrap();
        assert!(matches!(found, Availability::Ready(engine) if engine.adapter() == "idle/1"));
        assert!(announced(&runtime, &ctx) >= 1, "each wait is announced");
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn a_suspect_engine_is_not_launched_until_its_probe_passes() {
        let (runtime, _dir, ctx) = waiting(Duration::from_secs(30)).await;
        runtime.engines.mark_usable(KIND, Arc::new(Idle));
        runtime.engines.mark_suspect(KIND, "claude is not on PATH");
        let clearing = runtime.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            clearing.engines.clear_suspect(KIND);
        });
        let found = wait_for_engine(
            &runtime,
            &ctx,
            KIND,
            "claude-code",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(matches!(found, Availability::Ready(_)));
        assert!(announced(&runtime, &ctx) >= 1);
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn an_engine_still_missing_when_the_window_ends_fails_the_turn() {
        let (runtime, _dir, ctx) = waiting_with_session(Duration::from_millis(200)).await;
        let started = std::time::Instant::now();
        let found = wait_for_engine(
            &runtime,
            &ctx,
            KIND,
            "claude-code",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        let Availability::Unavailable(reason) = found else {
            panic!("the engine never came back");
        };
        assert!(reason.contains("not installed"), "{reason}");
        assert!(started.elapsed() >= Duration::from_millis(200));
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn an_engine_never_found_fails_a_new_agent_at_once() {
        let (runtime, _dir, ctx) = waiting(Duration::from_secs(3600)).await;
        let cancel = CancellationToken::new();
        let found = wait_for_engine(&runtime, &ctx, KIND, "claude-code", &cancel)
            .await
            .unwrap();
        assert!(
            matches!(found, Availability::Unavailable(ref reason) if reason.contains("not installed"))
        );
        assert_eq!(announced(&runtime, &ctx), 0, "nothing was waited for");
        runtime.shutdown().await;
    }

    /// A probe that never returns, as one queued behind a hung CLI's would.
    fn hang_probes_after(runtime: &Arc<Runtime>, after: Duration) {
        let holding = runtime.clone();
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            holding.engines.hang_rechecks();
        });
    }

    #[tokio::test]
    async fn a_hung_probe_ends_with_the_window() {
        for (window, hangs_after) in [(300, 50), (300, 0), (0, 0)] {
            let window = Duration::from_millis(window);
            let (runtime, _dir, ctx) = waiting_with_session(window).await;
            hang_probes_after(&runtime, Duration::from_millis(hangs_after));
            if hangs_after == 0 {
                // The hang is in place before the first probe starts.
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let started = std::time::Instant::now();
            let cancel = CancellationToken::new();
            let found = tokio::time::timeout(
                Duration::from_secs(5),
                wait_for_engine(&runtime, &ctx, KIND, "claude-code", &cancel),
            )
            .await
            .expect("the window bounds a probe that never returns, the first one too")
            .unwrap();
            assert!(
                matches!(found, Availability::Unavailable(ref reason) if reason.contains("not installed")),
                "the reason on record is kept"
            );
            assert!(started.elapsed() >= window);
            runtime.shutdown().await;
        }
    }

    #[tokio::test]
    async fn cancelling_ends_a_probe_still_running() {
        let (runtime, _dir, ctx) = waiting_with_session(Duration::from_secs(3600)).await;
        hang_probes_after(&runtime, Duration::ZERO);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let cancel = CancellationToken::new();
        let cancelling = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancelling.cancel();
        });
        let found = tokio::time::timeout(
            Duration::from_secs(5),
            wait_for_engine(&runtime, &ctx, KIND, "claude-code", &cancel),
        )
        .await
        .expect("cancelling does not wait for the probe");
        assert!(matches!(found.unwrap(), Availability::Cancelled));
        assert_eq!(announced(&runtime, &ctx), 0, "no probe finished to wait on");
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn a_zero_window_fails_at_once_and_cancelling_ends_the_wait() {
        let (runtime, _dir, ctx) = waiting_with_session(Duration::ZERO).await;
        let found = wait_for_engine(
            &runtime,
            &ctx,
            KIND,
            "claude-code",
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(matches!(found, Availability::Unavailable(_)));
        assert_eq!(announced(&runtime, &ctx), 0);
        runtime.shutdown().await;
        let (runtime, _dir, ctx) = waiting_with_session(Duration::from_secs(3600)).await;
        let cancel = CancellationToken::new();
        cancel.cancel();
        let found = wait_for_engine(&runtime, &ctx, KIND, "claude-code", &cancel)
            .await
            .unwrap();
        assert!(matches!(found, Availability::Cancelled));
        runtime.shutdown().await;
    }
}
