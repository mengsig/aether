//! What `aether run` says when the agent is held up and the event stream is
//! otherwise quiet: a low disk gate and an engine waiting out a bad network.

use super::sane;
use aether_core::disk::LowDisk;
use std::time::Duration;
use tokio::time::{Interval, MissedTickBehavior};

/// How long a gate may hold a new turn before `run` first looks for it.
const FIRST_CHECK: Duration = Duration::from_secs(3);
const CHECK_EVERY: Duration = Duration::from_secs(10);

/// Polls the daemon's status for the disk gate and reports only changes, so
/// a run that is never held up prints nothing extra.
pub(super) struct GateWatch {
    ticker: Interval,
    holding: bool,
}

impl GateWatch {
    pub(super) fn new() -> Self {
        let start = tokio::time::Instant::now() + FIRST_CHECK;
        let mut ticker = tokio::time::interval_at(start, CHECK_EVERY);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        Self {
            ticker,
            holding: false,
        }
    }

    pub(super) async fn tick(&mut self) {
        self.ticker.tick().await;
    }

    /// The line to print for the daemon's current `low_disk`, if the gate
    /// changed since the last look.
    pub(super) fn update(&mut self, low_disk: Option<&LowDisk>) -> Option<String> {
        let line = match (low_disk, self.holding) {
            (Some(low), false) => Some(low_disk_line(low)),
            (None, true) => Some("disk space is back above the floor; the run resumes".into()),
            _ => None,
        };
        self.holding = low_disk.is_some();
        line
    }
}

fn low_disk_line(low: &LowDisk) -> String {
    let gib = |bytes: u64| bytes as f64 / (1u64 << 30) as f64;
    format!(
        "waiting: disk is low, {:.1} GiB free on the filesystem of {}, floor {:.1} GiB; new turns wait until space is freed",
        gib(low.available_bytes),
        sane(&low.path),
        gib(low.floor_bytes)
    )
}

/// The line for an `engine.transient_retry` event, or `None` for any other.
pub(super) fn retry_line(name: &str, payload: &serde_json::Value) -> Option<String> {
    if name != "engine.transient_retry" {
        return None;
    }
    let text = |key: &str, default: &'static str| {
        payload[key]
            .as_str()
            .map_or_else(|| default.to_owned(), sane)
    };
    let secs = payload["delay_ms"].as_u64().unwrap_or(0).div_ceil(1000);
    let attempt = payload["attempt"].as_u64().unwrap_or(0);
    Some(format!(
        "waiting: {} hit a {} error; waiting for the {}, retry {attempt} in {secs}s",
        text("engine", "engine"),
        text("class", "transient"),
        text("waiting_for", "provider"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn low() -> LowDisk {
        LowDisk {
            path: "/store".into(),
            available_bytes: 1 << 30,
            floor_bytes: 5 << 30,
            since: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn the_gate_is_named_once_and_its_release_once() {
        let mut watch = GateWatch::new();
        assert_eq!(watch.update(None), None, "a normal run is quiet");
        let said = watch.update(Some(&low())).expect("gate named");
        assert!(
            said.contains("disk is low") && said.contains("/store"),
            "{said}"
        );
        assert_eq!(watch.update(Some(&low())), None, "not repeated");
        let freed = watch.update(None).expect("release named");
        assert!(freed.contains("resumes"), "{freed}");
        assert_eq!(watch.update(None), None);
    }

    #[test]
    fn a_transient_retry_event_is_printed_and_other_events_are_not() {
        let payload = serde_json::json!({
            "engine": "codex", "class": "network", "waiting_for": "network",
            "attempt": 2, "delay_ms": 4500,
        });
        let said = retry_line("engine.transient_retry", &payload).expect("printed");
        assert_eq!(
            said,
            "waiting: codex hit a network error; waiting for the network, retry 2 in 5s"
        );
        assert_eq!(retry_line("engine.interrupted", &payload), None);
    }
}
