use super::{ProviderUsage, QuotaWindow};
use crate::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Read-time projection of the latest report; unknown usage is always stale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProviderHeadroom {
    pub windows: Vec<WindowHeadroom>,
    pub observed_at: Option<Timestamp>,
    pub age_seconds: Option<u64>,
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WindowHeadroom {
    pub name: String,
    pub window_minutes: Option<u64>,
    pub remaining_percent: u64,
    pub resets_at: Option<Timestamp>,
    pub resets_in_seconds: Option<u64>,
}

impl Default for ProviderHeadroom {
    fn default() -> Self {
        Self {
            windows: Vec::new(),
            observed_at: None,
            age_seconds: None,
            stale: true,
        }
    }
}

impl ProviderHeadroom {
    pub fn from_usage(usage: Option<&ProviderUsage>, at: Timestamp, stale_after: Duration) -> Self {
        let Some(usage) = usage else {
            return Self::default();
        };
        let age = (at - usage.observed_at).to_std().unwrap_or_default();
        let windows: Vec<_> = usage
            .windows
            .iter()
            .map(|window| WindowHeadroom {
                name: window.name.clone(),
                window_minutes: window.quota.window_minutes,
                remaining_percent: 100_u64.saturating_sub(window.quota.used_percent),
                resets_at: window.quota.resets_at,
                resets_in_seconds: window
                    .quota
                    .resets_at
                    .map(|reset| (reset - at).num_seconds().max(0) as u64),
            })
            .collect();
        debug_assert_eq!(windows.len(), usage.windows.len());
        debug_assert!(windows.iter().all(|window| window.remaining_percent <= 100));
        Self {
            windows,
            observed_at: Some(usage.observed_at),
            age_seconds: Some(age.as_secs()),
            stale: usage.windows.is_empty() || age > stale_after,
        }
    }
}

impl QuotaWindow {
    /// UTC is explicit because a weekly reset may fall on another date.
    pub fn headroom_label(&self, name: &str) -> String {
        let label = self
            .window_label()
            .unwrap_or_else(|| name.replace('_', " "));
        let remaining = 100_u64.saturating_sub(self.used_percent);
        let reset = self.resets_at.map_or_else(
            || "reset unknown".to_owned(),
            |at| {
                format!(
                    "resets {} UTC ({})",
                    at.format("%H:%M"),
                    at.format("%Y-%m-%d")
                )
            },
        );
        format!("{label}: {remaining}% left, {reset}")
    }
}

#[cfg(test)]
mod tests;
