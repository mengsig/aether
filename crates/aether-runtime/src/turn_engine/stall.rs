//! Watchdog for native engine turns that go silent: the engine sent a model
//! request and nothing ever came back, which no timeout of its own ends.
use std::time::Duration;

use tokio::time::Instant;

use super::{Commit, EngineKind, EngineOutcome, EventKind, Result, Runtime, TurnCtx, json};

/// Recorded on every stall, so clients can show or subscribe to it.
pub(super) const STALLED_EVENT: &str = "agent.stalled";

/// How many times a stalled turn is interrupted and resumed before it fails.
pub(super) const MAX_RETRIES: u32 = 2;

/// Last time the engine showed a sign of life, and how long it may go quiet.
pub(super) struct Watchdog {
    timeout: Duration,
    last_activity: Instant,
    was_waiting: bool,
}

impl Watchdog {
    /// A zero timeout never fires.
    pub(super) fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            last_activity: Instant::now(),
            was_waiting: false,
        }
    }

    pub(super) fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    /// When the turn counts as stalled. `None` while the watchdog is off or
    /// the turn legitimately waits on something else (`waiting` is true).
    /// The clock restarts when such a wait ends: the engine only now gets
    /// to answer whatever it waited for.
    pub(super) fn deadline(&mut self, waiting: bool) -> Option<Instant> {
        if self.was_waiting && !waiting {
            self.touch();
        }
        self.was_waiting = waiting;
        if self.timeout.is_zero() || waiting {
            return None;
        }
        Some(self.last_activity + self.timeout)
    }

    pub(super) fn silent_for(&self) -> Duration {
        self.last_activity.elapsed()
    }
}

/// The outcome of a turn the watchdog ended.
pub(super) fn stalled_outcome(silent: Duration) -> EngineOutcome {
    debug_assert!(!silent.is_zero(), "a stall needs a silence");
    EngineOutcome {
        is_error: true,
        error: Some(format!(
            "engine stalled: no output for {}s while no tool was running",
            silent.as_secs()
        )),
        ..Default::default()
    }
}

/// What the terminal error says when the retries ran out.
pub(super) fn gave_up(silent: Duration, attempts: u32) -> String {
    format!(
        "stalled {attempts} times, silent for {}s at the last; the agent is kept: message it to \
         resume the session, or check the engine and its provider",
        silent.as_secs()
    )
}

fn problem_key(agent: impl std::fmt::Display) -> String {
    format!("engine.stalled.{agent}")
}

/// The engine answered again: `aether status` stops flagging the agent.
pub(super) fn clear_problem(runtime: &Runtime, ctx: &TurnCtx) {
    runtime.clear_problem(&problem_key(ctx.agent.id));
}

/// Announce a stall; `attempt` counts stalls in this turn, from 1. It also
/// stays in the daemon's problems, which `aether status` lists, until the
/// agent's engine runs again.
pub(super) fn record(
    runtime: &Runtime,
    ctx: &TurnCtx,
    engine: EngineKind,
    model: &str,
    silent: Duration,
    attempt: u32,
) -> Result<()> {
    assert!(attempt >= 1, "attempts count from 1");
    runtime.report_problem_about(
        crate::problems::Subject::Agent(ctx.agent.id),
        problem_key(ctx.agent.id),
        format!(
            "Agent {} stalled on {} ({model}): no output for {}s, attempt {attempt}; \
             message the agent to resume it",
            ctx.agent.id,
            engine.as_str(),
            silent.as_secs()
        ),
    );
    runtime.apply(Commit::new().event(ctx.draft(EventKind::Custom {
        name: STALLED_EVENT.into(),
        payload: json!({
            "agent": ctx.agent.id,
            "engine": engine.as_str(),
            "model": model,
            "silent_secs": silent.as_secs(),
            "attempt": attempt,
        }),
    })))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn the_deadline_follows_the_last_sign_of_life() {
        let mut dog = Watchdog::new(Duration::from_secs(60));
        let first = dog.deadline(false).unwrap();
        tokio::time::advance(Duration::from_secs(40)).await;
        dog.touch();
        assert_eq!(
            dog.deadline(false).unwrap() - first,
            Duration::from_secs(40)
        );
        assert_eq!(dog.silent_for(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn the_clock_restarts_when_a_wait_ends() {
        let mut dog = Watchdog::new(Duration::from_secs(60));
        assert!(dog.deadline(true).is_none());
        tokio::time::advance(Duration::from_secs(3600)).await;
        let deadline = dog.deadline(false).unwrap();
        assert!(deadline > Instant::now(), "a long wait is not silence");
    }

    #[test]
    fn a_waiting_turn_or_zero_timeout_never_stalls() {
        assert!(
            Watchdog::new(Duration::from_secs(60))
                .deadline(true)
                .is_none()
        );
        assert!(Watchdog::new(Duration::ZERO).deadline(false).is_none());
    }
}
