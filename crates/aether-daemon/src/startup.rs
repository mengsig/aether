//! How long each phase of a daemon start took, so a slow start names its cause.

use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) struct StartupClock {
    started: Instant,
    phase_started: Instant,
}

impl StartupClock {
    pub(crate) fn start() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            phase_started: now,
        }
    }

    /// Log the phase that just ended and start timing the next one.
    pub(crate) fn finished(&mut self, phase: &'static str) -> Duration {
        let now = Instant::now();
        let took = now.duration_since(self.phase_started);
        let total = now.duration_since(self.started);
        debug_assert!(took <= total, "a phase starts no earlier than the daemon");
        debug_assert!(!phase.is_empty(), "every phase has a name");
        tracing::info!(
            phase,
            phase_ms = took.as_millis() as u64,
            since_start_ms = total.as_millis() as u64,
            "startup phase finished"
        );
        self.phase_started = now;
        took
    }
}

#[cfg(test)]
mod tests {
    use super::StartupClock;

    #[test]
    fn phases_add_up_to_the_time_since_start() {
        let mut clock = StartupClock::start();
        let first = clock.finished("first");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let second = clock.finished("second");
        assert!(second >= std::time::Duration::from_millis(5));
        assert!(first + second <= clock.started.elapsed());
    }
}
