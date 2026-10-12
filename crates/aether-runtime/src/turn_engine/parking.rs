//! Native engine parking.
use super::{
    AgentId, Arc, AtomicBool, BTreeSet, CancellationToken, Commit, EngineKind, EngineOutcome,
    EventKind, Next, Ordering, Result, Runtime, TurnCtx, json, mpsc,
};
use aether_engine::EngineInput;

/// After background tasks finish, how long to wait for the engine's next
/// result before treating the last one as final.
pub(super) const BACKGROUND_GRACE: std::time::Duration = std::time::Duration::from_secs(90);

/// After the engine's result, how long a steered message may wait for its
/// acknowledgement before the turn ends and the message is kept for the next.
pub(super) const STEER_GRACE: std::time::Duration = std::time::Duration::from_secs(45);

/// After the runtime stops a parked engine, how long its final result may
/// take to arrive, in [`REAP_SLICE`]s: 5 s. The turn is not recorded until the
/// stream closes or this expires, so the next turn cannot resume a session
/// still being killed.
const REAP_SLICES: u32 = 50;

/// One step of [`ReapGrace`]: the most of the grace a host stall can spend.
const REAP_SLICE: std::time::Duration = std::time::Duration::from_millis(100);

/// The reap grace counted in slices the runtime was awake for, not as one
/// deadline. A stall (SIGSTOP, a starved host) freezes the engine's own
/// cleanup too, so it spends one slice at most instead of the whole grace.
pub(super) struct ReapGrace {
    slices_left: u32,
    slice_ends: tokio::time::Instant,
}

impl ReapGrace {
    pub(super) fn start() -> Self {
        Self {
            slices_left: REAP_SLICES,
            slice_ends: tokio::time::Instant::now() + REAP_SLICE,
        }
    }

    /// Resolves once every slice has passed. Cancel-safe: a `select!` that
    /// drops it keeps the slices already spent.
    pub(super) async fn spent(&mut self) {
        while self.slices_left > 0 {
            tokio::time::sleep_until(self.slice_ends).await;
            self.slices_left -= 1;
            // From when the runtime woke, so a stall ends only this slice.
            self.slice_ends = tokio::time::Instant::now() + REAP_SLICE;
        }
    }
}

/// The default answer during a park's wind-down, served or refused: the turn's
/// end is recorded, so every answer carries the stop. A turn parked on a
/// question refuses `complete_task`/`fail_task` with a specific message
/// instead - unless the agent is `long_lived`, whose settle is admitted.
pub(super) const TURN_IS_OVER: &str = concat!(
    "This turn is over: a harness tool already ended it. Stop now and make no ",
    "further tool calls; you will be woken when there is more to do."
);

/// Backgrounded tool ids one turn tracks, and how much of an engine-chosen
/// name or error is kept. All of it comes from the engine's own stdout, so
/// all of it is bounded before it reaches runtime state or a durable event.
pub(super) const MAX_BACKGROUND_IDS: usize = 32;

pub(super) const MAX_ENGINE_NAME: usize = 64;

pub(super) const MAX_ENGINE_ERROR: usize = 1_024;

/// How many engine-chosen names the wind-down event lists; enough to name
/// the pattern.
const MAX_PARKED_NAMES: usize = 8;

/// A provider or context limit named in native engine prose: no retry can answer it.
pub(super) fn names_exhaustion(error: &str) -> bool {
    crate::exhaustion::recoverable_by_fallback(crate::exhaustion::engine_error_class(error))
}

/// Engine-chosen text, cut to a length the runtime is willing to hold.
pub(super) fn clamp(text: &str, max: usize) -> String {
    aether_model::http::safe_provider_text(text, max)
        .chars()
        .take(max)
        .collect()
}

/// Whether the process behind a parked turn has already been stopped.
pub(super) fn stopping(park: &Option<Park>) -> bool {
    park.as_ref().is_some_and(Park::killed)
}

/// The wind-down after a harness action ended the turn: what the engine was
/// still doing, what it tried to do afterwards, and how it finally stopped.
/// Recorded as an `engine.parked` event, which is the only durable trace an
/// operator has of an engine that does not honour the stop.
#[derive(Debug)]
pub(super) struct Park {
    /// The runtime stopped the process rather than the engine ending itself.
    /// Shared with the stop timer, which is what usually sets it.
    pub(super) killed: Arc<AtomicBool>,
    /// The armed stop, so a turn that ends before it fires can call it off.
    pub(super) stop: Option<tokio::task::AbortHandle>,
    /// Harness calls the turn refused after its end was decided.
    pub(super) refused: usize,
    /// The first few of those, for the event.
    pub(super) refused_names: Vec<String>,
    /// Ids of the tools the engine had backgrounded when the turn ended.
    pub(super) background: Vec<String>,
}

impl Park {
    pub(super) fn started(background: &BTreeSet<String>) -> Self {
        Self {
            killed: Arc::new(AtomicBool::new(false)),
            stop: None,
            refused: 0,
            refused_names: Vec::new(),
            background: background.iter().take(MAX_PARKED_NAMES).cloned().collect(),
        }
    }

    pub(super) fn killed(&self) -> bool {
        self.killed.load(Ordering::SeqCst)
    }

    /// Arm the stop on its own timer. The drive loop is not the enforcer: a
    /// parked engine can hold it inside a bridge call for as long as that
    /// call takes, and the grace has to bind anyway.
    pub(super) fn arm_stop(
        &mut self,
        at: tokio::time::Instant,
        agent: AgentId,
        cancel: CancellationToken,
        fired: mpsc::Sender<()>,
    ) {
        self.disarm();
        let killed = self.killed.clone();
        self.stop = Some(
            tokio::spawn(async move {
                tokio::time::sleep_until(at).await;
                tracing::warn!(%agent, "engine did not end its turn after it parked; stopping the process");
                // Ordered before the cancel: a Done can now arrive in the same
                // instant as the cancellation, and `stopping(&park)` (the
                // drive loop's reap gate) has to see this store already done,
                // or a parked turn falls into the plain-Done arm unreaped.
                killed.store(true, Ordering::SeqCst);
                cancel.cancel();
                // Capacity 1, sent once: the loop only needs the wake.
                let _ = fired.try_send(());
            })
            .abort_handle(),
        );
    }

    /// Stop the process now: there is nothing left for it to report.
    pub(super) fn stop_now(&mut self, cancel: &CancellationToken) {
        self.disarm();
        // Same ordering requirement as `arm_stop`: stored before the cancel,
        // so `stopping(&park)` sees the park before the Done it races.
        self.killed.store(true, Ordering::SeqCst);
        cancel.cancel();
    }

    pub(super) fn disarm(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop.abort();
        }
    }

    pub(super) fn refused(&mut self, name: &str) {
        self.refused += 1;
        if self.refused_names.len() < MAX_PARKED_NAMES {
            self.refused_names.push(clamp(name, MAX_ENGINE_NAME));
        }
    }
}

impl Drop for Park {
    /// A turn that ended early (an error out of the drive loop) must not
    /// leave a timer behind that stops someone else's process.
    fn drop(&mut self) {
        self.disarm();
    }
}

/// Begin the wind-down and arm an independent hard-stop deadline. Claude's
/// soft interrupt can wait for its tool reply acknowledgement; that does not
/// extend this deadline. A zero grace is already stopped when this returns.
#[allow(clippy::too_many_arguments)]
pub(super) fn start_park(
    agent: AgentId,
    grace: std::time::Duration,
    background: &BTreeSet<String>,
    input: &mut Option<mpsc::Sender<EngineInput>>,
    stop: Option<&mpsc::Sender<std::time::Duration>>,
    cancel: &CancellationToken,
    fired: mpsc::Sender<()>,
    defer_interrupt: bool,
) -> Park {
    if !defer_interrupt || grace.is_zero() {
        ask_to_stop(input, stop, grace);
    }
    let mut park = Park::started(background);
    if grace.is_zero() {
        park.stop_now(cancel);
    } else {
        park.arm_stop(
            tokio::time::Instant::now() + grace,
            agent,
            cancel.clone(),
            fired,
        );
    }
    park
}

/// How long a parked engine gets before its process is stopped. Terminal
/// decisions retain a bounded window for native final usage; runtime wait
/// evaluation holds their result behind the producing turn's in-flight slot.
/// Tools the engine backgrounded get the longer background
/// grace - killing them would lose their results - but never more than the
/// operator allowed: that set is filled from engine stdout, so an uncapped
/// grace there would be one the engine chose for itself.
pub(super) fn park_grace(
    next: &Option<Next>,
    background_running: bool,
    configured: std::time::Duration,
) -> std::time::Duration {
    match next {
        Some(Next::Completed(_) | Next::Failed(_)) => {
            configured.min(crate::DEFAULT_ENGINE_PARK_GRACE)
        }
        _ if background_running => BACKGROUND_GRACE.min(configured),
        _ => configured,
    }
}

/// Ask the engine to end the turn the way it supports: closing stdin, which
/// a CLI that exits on it honours, plus its own stop verb when it has one.
/// Queue the native interrupt before closing input so EOF cannot race it.
/// `grace` is how
/// long the runtime will wait before stopping the process itself, so an
/// adapter with its own wind-down timer does not outlast it.
pub(super) fn ask_to_stop(
    input: &mut Option<mpsc::Sender<EngineInput>>,
    stop: Option<&mpsc::Sender<std::time::Duration>>,
    grace: std::time::Duration,
) {
    if let Some(stop) = stop {
        // Capacity 1 and asked once: a full channel is a stop already sent.
        let _ = stop.try_send(grace);
    }
    drop(input.take());
}

/// Who ended the run. An adapter that force-killed the process when its own
/// grace expired did not have the engine stop of its own accord, and must
/// not be recorded as if it had. Both can be true - the runtime asked for the
/// stop and the adapter still had to force it - and naming only one of them
/// hides half of what an operator is looking at.
pub(super) fn stopped_by(park: &Park, outcome: &EngineOutcome) -> &'static str {
    match (park.killed(), outcome.forced) {
        (true, true) => "runtime+adapter",
        (true, false) => "runtime",
        (false, true) => "adapter",
        (false, false) => "engine",
    }
}

/// The durable trace of a wind-down. A stop the runtime had to force, an
/// error the engine reported afterwards, and the calls it kept making are
/// exactly what an operator needs to see an engine misbehaving.
pub(super) fn record_park(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    kind: EngineKind,
    park: &Park,
    outcome: &EngineOutcome,
) -> Result<()> {
    let mut commit = Commit::new().event(ctx.draft(EventKind::Custom {
        name: "engine.parked".into(),
        payload: json!({
            "engine": kind.as_str(),
            "stopped_by": stopped_by(park, outcome),
            "interrupted": outcome.interrupted,
            "is_error": outcome.is_error,
            "error": outcome.error.as_deref().map(|e| clamp(e, MAX_ENGINE_ERROR)),
            "refused_calls": park.refused,
            "refused_first": park.refused_names,
            "background_ids": park.background,
        }),
    }));
    // A boundary the adapter enforced mid-wind-down (a kill-switch bypass)
    // still has to be visible under its own name.
    if outcome.interrupted {
        commit.push_event(ctx.draft(EventKind::Custom {
            name: "engine.interrupted".into(),
            payload: json!({
                "engine": kind.as_str(),
                "reason": outcome.error.as_deref().map(|e| clamp(e, MAX_ENGINE_ERROR)),
            }),
        }));
    }
    runtime.apply(commit)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn terminal_reporting_respects_a_shorter_limit_and_caps_a_longer_one() {
        for next in [Next::Completed(None), Next::Failed("failed".into())] {
            assert_eq!(
                park_grace(&Some(next.clone()), false, Duration::from_millis(50)),
                Duration::from_millis(50)
            );
            assert_eq!(
                park_grace(&Some(next.clone()), true, Duration::from_secs(3600)),
                crate::DEFAULT_ENGINE_PARK_GRACE
            );
            assert_eq!(
                park_grace(&Some(next), false, Duration::ZERO),
                Duration::ZERO
            );
        }
    }

    const REAP_GRACE: Duration = Duration::from_secs(5);

    #[tokio::test(start_paused = true)]
    async fn the_reap_grace_lasts_its_length_when_nothing_stalls() {
        let started = tokio::time::Instant::now();
        ReapGrace::start().spent().await;
        assert_eq!(started.elapsed(), REAP_GRACE);
    }

    #[tokio::test(start_paused = true)]
    async fn a_host_stall_spends_one_slice_of_the_reap_grace() {
        let mut grace = ReapGrace::start();
        // Past the whole grace while nothing runs.
        tokio::time::advance(REAP_GRACE * 10).await;
        let woke = tokio::time::Instant::now();
        grace.spent().await;
        assert_eq!(woke.elapsed(), REAP_GRACE - REAP_SLICE);
    }

    #[tokio::test(start_paused = true)]
    async fn a_dropped_wait_keeps_the_slices_it_spent() {
        let mut grace = ReapGrace::start();
        let started = tokio::time::Instant::now();
        let early = tokio::time::timeout(REAP_GRACE / 2, grace.spent()).await;
        assert!(early.is_err());
        grace.spent().await;
        assert_eq!(started.elapsed(), REAP_GRACE);
    }
}
