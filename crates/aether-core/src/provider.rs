//! Provider scheduling health: how many turns a provider is carrying, what
//! it is allowed to carry, whether it has asked us to slow down, what it has
//! been failing with, and where a turn last went when it gave up on it.
//!
//! Every projection (status, `aether providers`, the TUI) reads these
//! values; the scheduler is the single writer.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{ModelErrorClass, ModelRef};
use crate::time::Timestamp;

mod headroom;
pub use headroom::{ProviderHeadroom, WindowHeadroom};

/// A subscription usage window as an engine reported it. Opaque JSON on the
/// wire from the CLI; normalized here so every reader agrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct QuotaWindow {
    /// How much of the window is spent, 0-100 (clamped).
    pub used_percent: u64,
    /// Length of the window in minutes, when the engine says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<Timestamp>,
}

impl QuotaWindow {
    /// Codex reports `primary`/`secondary` windows with `usedPercent`;
    /// Claude Code reports `unifiedWindows.five_hour/seven_day` with a
    /// `utilization` fraction. The shorter window is what matters now.
    pub fn from_engine_payload(payload: &Value) -> Option<Self> {
        for name in ["primary", "secondary"] {
            let Some(window) = payload.get(name) else {
                continue;
            };
            let Some(percent) = window.get("usedPercent").and_then(Value::as_f64) else {
                continue;
            };
            return Some(Self {
                used_percent: percent.round().clamp(0.0, 100.0) as u64,
                window_minutes: window.get("windowDurationMins").and_then(Value::as_u64),
                resets_at: reset_time(window),
            });
        }
        let windows = payload.get("unifiedWindows")?;
        for (name, minutes) in [("five_hour", 300), ("seven_day", 10_080)] {
            let Some(utilization) = windows
                .get(name)
                .and_then(|w| w.get("utilization"))
                .and_then(Value::as_f64)
            else {
                continue;
            };
            return Some(Self {
                used_percent: (utilization * 100.0).round().clamp(0.0, 100.0) as u64,
                window_minutes: Some(minutes),
                resets_at: reset_time(&windows[name]),
            });
        }
        None
    }

    /// `5h`, `7d`, `30m`.
    pub fn window_label(&self) -> Option<String> {
        let minutes = self.window_minutes?;
        Some(if minutes % 1440 == 0 {
            format!("{}d", minutes / 1440)
        } else if minutes % 60 == 0 {
            format!("{}h", minutes / 60)
        } else {
            format!("{minutes}m")
        })
    }
}

fn reset_time(window: &Value) -> Option<Timestamp> {
    Timestamp::from_timestamp(window.get("resetsAt")?.as_i64()?, 0)
}

/// One named account limit, including limits outside the preferred scheduling window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct UsageWindow {
    pub name: String,
    pub quota: QuotaWindow,
}

/// Latest normalized account report. The observation time is not a reset time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProviderUsage {
    pub observed_at: Timestamp,
    pub windows: Vec<UsageWindow>,
}

impl ProviderUsage {
    pub fn from_engine_payload(payload: &Value, observed_at: Timestamp) -> Option<Self> {
        // Require at least one valid scheduling window.
        let preferred = QuotaWindow::from_engine_payload(payload)?;
        let mut windows = codex_windows(payload);
        if windows.is_empty() {
            windows = claude_windows(payload)?;
        }
        debug_assert!(!windows.is_empty());
        debug_assert_eq!(windows.first().map(|w| w.quota), Some(preferred));
        Some(Self {
            observed_at,
            windows,
        })
    }

    pub fn preferred_window(&self) -> Option<QuotaWindow> {
        self.windows.first().map(|window| window.quota)
    }
}

fn codex_windows(payload: &Value) -> Vec<UsageWindow> {
    let mut windows = Vec::new();
    for name in ["primary", "secondary"] {
        let Some(value) = payload.get(name) else {
            continue;
        };
        let Some(percent) = value.get("usedPercent").and_then(Value::as_f64) else {
            continue;
        };
        windows.push(UsageWindow {
            name: name.into(),
            quota: QuotaWindow {
                used_percent: percent.round().clamp(0.0, 100.0) as u64,
                window_minutes: value.get("windowDurationMins").and_then(Value::as_u64),
                resets_at: reset_time(value),
            },
        });
    }
    windows
}

fn claude_windows(payload: &Value) -> Option<Vec<UsageWindow>> {
    let reported = payload.get("unifiedWindows")?.as_object()?;
    let mut names: Vec<_> = reported.keys().collect();
    names.sort_by_key(|name| match name.as_str() {
        "five_hour" => (0, name.as_str()),
        "seven_day" => (1, name.as_str()),
        _ => (2, name.as_str()),
    });
    let mut windows = Vec::new();
    for name in names {
        let Some(utilization) = reported[name].get("utilization").and_then(Value::as_f64) else {
            continue;
        };
        windows.push(UsageWindow {
            name: name.clone(),
            quota: QuotaWindow {
                used_percent: (utilization * 100.0).round().clamp(0.0, 100.0) as u64,
                window_minutes: match name.as_str() {
                    "five_hour" => Some(300),
                    "seven_day" => Some(10_080),
                    _ => None,
                },
                resets_at: reset_time(&reported[name]),
            },
        });
    }
    Some(windows)
}

/// Why a provider is being avoided, and until when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProviderBackoff {
    pub reason: String,
    pub until: Timestamp,
}

/// One normalized error class a provider returned recently, and how often.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ErrorTally {
    pub class: ModelErrorClass,
    /// Times seen among the samples still retained.
    pub count: u32,
    /// When this class was last seen.
    pub last_at: Timestamp,
}

/// The last time a turn gave up on a provider and moved to the next link of
/// its chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FallbackRecord {
    /// The link that took the turn over.
    pub to: ModelRef,
    /// Why the turn left, in one line.
    pub reason: String,
    /// The normalized class that caused the move, when an error did. A link
    /// skipped because its provider was already backed off carries none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<ModelErrorClass>,
    pub at: Timestamp,
}

/// A bounded, time-limited ring of a provider's newest failures, aggregated
/// on read.
///
/// Live state, like backoff and quota: it starts empty on every daemon start
/// and says how the provider is behaving now, not what it ever did. Samples
/// older than [`ErrorWindow::WINDOW`] leave, so a provider that failed this
/// morning and has been healthy since reads as healthy. The event log is the
/// durable record.
#[derive(Debug, Clone, Default)]
pub struct ErrorWindow {
    samples: VecDeque<(ModelErrorClass, Timestamp)>,
}

impl ErrorWindow {
    /// Newest samples kept per provider: enough to show a pattern, small
    /// enough to hand to every projection on every read.
    pub const CAPACITY: usize = 20;

    /// How long a failure counts as current. Health answers "why is this
    /// provider not carrying my work right now", so an older sample is
    /// dropped rather than shown as if it were still true.
    pub const WINDOW: chrono::Duration = chrono::Duration::minutes(15);

    pub fn record(&mut self, class: ModelErrorClass, at: Timestamp) {
        self.expire(at);
        if self.samples.len() >= Self::CAPACITY {
            self.samples.pop_front();
        }
        self.samples.push_back((class, at));
    }

    /// Samples arrive in time order, so the aged-out ones are the oldest.
    fn expire(&mut self, now: Timestamp) {
        let cutoff = now - Self::WINDOW;
        while self.samples.front().is_some_and(|(_, at)| *at < cutoff) {
            self.samples.pop_front();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// One tally per class seen inside the window, most recently seen first.
    pub fn tallies(&self, now: Timestamp) -> Vec<ErrorTally> {
        let cutoff = now - Self::WINDOW;
        let mut out: Vec<ErrorTally> = Vec::new();
        for (class, at) in self.samples.iter().filter(|(_, at)| *at >= cutoff) {
            match out.iter_mut().find(|t| t.class == *class) {
                Some(tally) => {
                    tally.count += 1;
                    tally.last_at = *at;
                }
                None => out.push(ErrorTally {
                    class: *class,
                    count: 1,
                    last_at: *at,
                }),
            }
        }
        out.sort_by_key(|t| std::cmp::Reverse(t.last_at));
        out
    }
}

/// One provider's scheduling picture. Derived state: the scheduler rebuilds
/// it from the turns in flight and the feedback providers have given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProviderHealth {
    pub provider: String,
    /// Model turns in flight on this provider right now.
    pub running: u32,
    /// Live agents parked on this provider's exhaustion, waiting to probe it.
    #[serde(default)]
    pub suspended_agents: u32,
    /// Configured cap on concurrent turns; `None` leaves it to the global limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Set while the scheduler is holding turns back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff: Option<ProviderBackoff>,
    /// Latest subscription window the provider reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<QuotaWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ProviderUsage>,
    /// Classes this provider has been failing with lately, newest first.
    /// Engines report a message rather than a class, so these cover the
    /// API-model turn path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_errors: Vec<ErrorTally>,
    /// Where a turn went the last time it gave up on this provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fallback: Option<FallbackRecord>,
}

impl ProviderHealth {
    /// A provider carrying nothing, with no feedback yet: what every
    /// configured provider looks like on a fresh daemon.
    pub fn idle(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            running: 0,
            suspended_agents: 0,
            limit: None,
            backoff: None,
            quota: None,
            usage: None,
            recent_errors: Vec::new(),
            last_fallback: None,
        }
    }

    pub fn is_healthy(&self) -> bool {
        self.backoff.is_none()
    }

    /// Whether another turn would exceed the configured per-provider cap.
    pub fn at_limit(&self) -> bool {
        self.limit.is_some_and(|l| self.running >= l)
    }

    /// Whether this provider has anything current to say: work in flight, a
    /// hold, a reported window, or feedback inside [`ErrorWindow::WINDOW`].
    /// A listing over the registry shows every provider regardless; `status`
    /// shows only these.
    pub fn has_live_feedback(&self, now: Timestamp) -> bool {
        self.running > 0
            || self.suspended_agents > 0
            || self.backoff.is_some()
            || self.quota.is_some()
            || !self.recent_errors.is_empty()
            || self
                .last_fallback
                .as_ref()
                .is_some_and(|f| now - f.at < ErrorWindow::WINDOW)
    }

    /// What this provider has been failing with lately and when it last did,
    /// as one line. Shared so the CLI and the TUI cannot drift.
    pub fn error_summary(&self) -> Option<String> {
        let newest = self.recent_errors.first()?;
        let classes: Vec<String> = self
            .recent_errors
            .iter()
            .map(|e| format!("{}x{}", e.class.as_str(), e.count))
            .collect();
        Some(format!(
            "{} (last {})",
            classes.join(" "),
            newest.last_at.format("%H:%M:%S")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn account_reports_keep_both_windows_without_changing_the_preferred_one() {
        for payload in [
            json!({"primary":{"usedPercent":42,"windowDurationMins":300},"secondary":{"usedPercent":73,"windowDurationMins":10080}}),
            json!({"unifiedWindows":{"five_hour":{"utilization":0.42},"seven_day":{"utilization":0.73}}}),
        ] {
            let report = ProviderUsage::from_engine_payload(&payload, crate::time::now()).unwrap();
            assert_eq!(report.windows.len(), 2);
            assert_eq!(
                report.preferred_window(),
                QuotaWindow::from_engine_payload(&payload)
            );
            assert_eq!(report.windows[1].quota.used_percent, 73);
            assert_eq!(
                report.windows[1].quota.window_label().as_deref(),
                Some("7d")
            );
            let roundtrip: ProviderUsage =
                serde_json::from_value(serde_json::to_value(&report).unwrap()).unwrap();
            assert_eq!(report, roundtrip);
        }
    }

    #[test]
    fn account_reports_read_legacy_quota_events() {
        let event: crate::event::EventKind = serde_json::from_value(json!({
            "type":"provider.quota", "provider":"codex", "window":{"used_percent":42,"window_minutes":300}
        })).unwrap();
        assert!(
            matches!(event, crate::event::EventKind::ProviderQuota { usage: None, window, .. } if window.used_percent == 42)
        );
    }

    #[test]
    fn account_reports_do_not_invent_missing_usage() {
        for payload in [
            json!({}),
            json!({"primary":{"usedPercent":null}}),
            json!({"unifiedWindows":{"five_hour":{"utilization":"unknown"}}}),
        ] {
            assert!(ProviderUsage::from_engine_payload(&payload, crate::time::now()).is_none());
        }
        let payload = json!({"primary":{"usedPercent":12.5},"secondary":null});
        let report = ProviderUsage::from_engine_payload(&payload, crate::time::now()).unwrap();
        assert_eq!(report.windows.len(), 1);
        assert_eq!(report.windows[0].quota.used_percent, 13);
        assert_eq!(report.windows[0].quota.window_minutes, None);
    }

    #[test]
    fn codex_and_claude_payloads_normalize() {
        let codex = QuotaWindow::from_engine_payload(
            &json!({"primary": {"usedPercent": 73, "windowDurationMins": 300}}),
        )
        .expect("codex window");
        assert_eq!(codex.used_percent, 73);
        assert_eq!(codex.window_label().as_deref(), Some("5h"));

        let claude = QuotaWindow::from_engine_payload(
            &json!({"unifiedWindows": {"five_hour": {"utilization": 0.965}}}),
        )
        .expect("claude window");
        assert_eq!(claude.used_percent, 97);
        assert_eq!(claude.window_label().as_deref(), Some("5h"));

        let weekly = QuotaWindow::from_engine_payload(
            &json!({"unifiedWindows": {"seven_day": {"utilization": 0.5}}}),
        )
        .expect("weekly window");
        assert_eq!(weekly.window_label().as_deref(), Some("7d"));

        assert!(QuotaWindow::from_engine_payload(&json!({"nothing": true})).is_none());
    }

    #[test]
    fn a_fractional_used_percent_still_reads() {
        let window = QuotaWindow::from_engine_payload(&json!({"primary": {"usedPercent": 96.4}}))
            .expect("a float window is still a window");
        assert_eq!(window.used_percent, 96);
    }

    #[test]
    fn percentages_are_clamped() {
        let over = QuotaWindow::from_engine_payload(
            &json!({"unifiedWindows": {"five_hour": {"utilization": 1.4}}}),
        )
        .expect("window");
        assert_eq!(over.used_percent, 100);
    }

    #[test]
    fn limits_and_health() {
        let mut health = ProviderHealth {
            running: 2,
            limit: Some(2),
            ..ProviderHealth::idle("anthropic")
        };
        assert!(health.is_healthy());
        assert!(health.at_limit());
        health.limit = None;
        assert!(!health.at_limit(), "no cap means the global limit decides");
    }

    #[test]
    fn the_error_window_tallies_newest_class_first() {
        let base = Timestamp::from_timestamp_nanos(0);
        let mut window = ErrorWindow::default();
        assert!(window.tallies(base).is_empty());
        window.record(ModelErrorClass::RateLimit, base);
        window.record(ModelErrorClass::Server, base + chrono::Duration::seconds(1));
        window.record(
            ModelErrorClass::RateLimit,
            base + chrono::Duration::seconds(2),
        );

        let tallies = window.tallies(base + chrono::Duration::seconds(2));
        assert_eq!(tallies.len(), 2, "one row per class");
        assert_eq!(tallies[0].class, ModelErrorClass::RateLimit);
        assert_eq!(tallies[0].count, 2);
        assert_eq!(tallies[0].last_at, base + chrono::Duration::seconds(2));
        assert_eq!(tallies[1].class, ModelErrorClass::Server);
        assert_eq!(tallies[1].count, 1);
    }

    #[test]
    fn the_error_window_keeps_only_the_newest_samples() {
        let base = Timestamp::from_timestamp_nanos(0);
        let mut window = ErrorWindow::default();
        window.record(ModelErrorClass::Auth, base);
        for i in 0..ErrorWindow::CAPACITY {
            window.record(
                ModelErrorClass::Server,
                base + chrono::Duration::seconds(i as i64 + 1),
            );
        }
        let tallies =
            window.tallies(base + chrono::Duration::seconds(ErrorWindow::CAPACITY as i64));
        assert_eq!(
            tallies.len(),
            1,
            "the oldest sample is dropped, so its class leaves with it"
        );
        assert_eq!(tallies[0].count, ErrorWindow::CAPACITY as u32);
    }

    #[test]
    fn the_error_window_ages_samples_out() {
        let base = Timestamp::from_timestamp_nanos(0);
        let mut window = ErrorWindow::default();
        window.record(ModelErrorClass::RateLimit, base);
        let later = base + ErrorWindow::WINDOW + chrono::Duration::seconds(1);
        assert!(
            window.tallies(later).is_empty(),
            "a failure older than the window is not what the provider is doing now"
        );

        window.record(ModelErrorClass::Server, later);
        let tallies = window.tallies(later);
        assert_eq!(tallies.len(), 1, "recording also drops what aged out");
        assert_eq!(tallies[0].class, ModelErrorClass::Server);
    }

    #[test]
    fn the_error_summary_says_when_it_last_happened() {
        let now = Timestamp::from_timestamp_nanos(0) + chrono::Duration::hours(9);
        let mut health = ProviderHealth::idle("anthropic");
        assert_eq!(health.error_summary(), None);
        health.recent_errors = vec![
            ErrorTally {
                class: ModelErrorClass::RateLimit,
                count: 3,
                last_at: now,
            },
            ErrorTally {
                class: ModelErrorClass::Server,
                count: 1,
                last_at: now - chrono::Duration::minutes(2),
            },
        ];
        assert_eq!(
            health.error_summary().as_deref(),
            Some("rate_limitx3 serverx1 (last 09:00:00)"),
            "a count with no time reads as if it were still happening"
        );
    }

    #[test]
    fn health_with_only_stale_feedback_is_not_live() {
        let now = Timestamp::from_timestamp_nanos(0) + chrono::Duration::hours(8);
        let mut health = ProviderHealth::idle("groq");
        assert!(!health.has_live_feedback(now));
        health.last_fallback = Some(FallbackRecord {
            to: ModelRef::new("openai", "gpt-5"),
            reason: "rate limited".into(),
            class: Some(ModelErrorClass::RateLimit),
            at: now - ErrorWindow::WINDOW - chrono::Duration::seconds(1),
        });
        assert!(
            !health.has_live_feedback(now),
            "a fallback older than the window is history, not health"
        );
        health.last_fallback.as_mut().expect("set").at = now;
        assert!(health.has_live_feedback(now));
    }

    #[test]
    fn malformed_primary_preserves_valid_secondary_window() {
        for malformed in [
            json!({"usedPercent":"unknown"}),
            json!({"usedPercent":null}),
            json!({}),
            Value::Null,
        ] {
            let payload = json!({"primary":malformed,"secondary":{"usedPercent":73,"windowDurationMins":10080}});
            let quota = QuotaWindow::from_engine_payload(&payload).unwrap();
            assert_eq!(quota.used_percent, 73);
            let report = ProviderUsage::from_engine_payload(&payload, crate::time::now()).unwrap();
            assert_eq!(report.windows.len(), 1);
            assert_eq!(report.windows[0].name, "secondary");
            assert_eq!(report.preferred_window(), Some(quota));
        }
    }

    #[test]
    fn malformed_claude_short_window_preserves_valid_weekly_window() {
        let payload = json!({"unifiedWindows":{"five_hour":{"utilization":"unknown"},"seven_day":{"utilization":0.73}}});
        let quota = QuotaWindow::from_engine_payload(&payload).unwrap();
        assert_eq!(quota.used_percent, 73);
        let report = ProviderUsage::from_engine_payload(&payload, crate::time::now()).unwrap();
        assert_eq!(report.windows.len(), 1);
        assert_eq!(report.preferred_window(), Some(quota));
        assert!(
            ProviderUsage::from_engine_payload(
                &json!({"primary":{"usedPercent":"unknown"},"secondary":{"usedPercent":"unknown"}}),
                crate::time::now()
            )
            .is_none()
        );
    }
}
