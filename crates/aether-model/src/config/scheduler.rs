use aether_core::disk::DiskFloor;
use serde::{Deserialize, Serialize};

// These are cost guards: a typo'd key (e.g. `max_concurrent_engine_turn`)
// must not load clean and silently cap nothing.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SchedulerSection {
    /// Global model-turn cap; the CLI can override this value.
    pub max_concurrent_turns: Option<usize>,
    /// Optional timed check-in for task/message waits; unset waits for events.
    /// Approval waits are never bounded.
    pub wait_timeout_minutes: Option<u64>,
    /// Grace an engine gets to end a turn a harness tool already ended,
    /// before its process is stopped; unset means 20 seconds.
    pub engine_park_grace_secs: Option<u64>,
    /// Cap on simultaneous engine (Claude Code, Codex) turns across the
    /// daemon; unset means unlimited, bounded only by the limits above.
    pub max_concurrent_engine_turns: Option<usize>,
    /// Dollars one run may spend before new delegations are refused; unset
    /// means no run budget.
    pub run_budget_usd: Option<f64>,
    /// Dollars at which a run's live agents are interrupted and no new turn
    /// is admitted; unset means no hard stop.
    pub run_hard_stop_usd: Option<f64>,
    /// Tokens at which the same stop applies, for engines that report no
    /// dollar cost; unset means no token stop.
    pub run_hard_stop_tokens: Option<u64>,
    /// How long an engine turn keeps waiting out network, timeout, capacity
    /// and server failures before the agent fails; unset means two hours.
    pub engine_transient_retry_minutes: Option<u64>,
    /// How long an engine turn may produce no output, with no tool running,
    /// before it is treated as stalled and retried; unset means 15 minutes,
    /// 0 turns the watchdog off.
    pub engine_stall_minutes: Option<u64>,
    /// Free space, in MiB, the data directory's filesystem keeps before new
    /// turns wait; unset means the smaller of 5 GiB and 2%, 0 never waits.
    pub min_free_disk_mib: Option<u64>,
}

/// A year. Past this the value is a mistake, not a policy, and the seconds
/// conversion starts to overflow.
pub const MAX_WAIT_TIMEOUT_MINUTES: u64 = 365 * 24 * 60;

/// An hour. A parked engine is being paid for the whole grace, so past this
/// the value is a mistake rather than a policy.
pub const MAX_ENGINE_PARK_GRACE_SECS: u64 = 60 * 60;

/// A week. Past this an agent that can never reach its provider is hung,
/// not resilient.
pub const MAX_ENGINE_TRANSIENT_RETRY_MINUTES: u64 = 7 * 24 * 60;

/// A week. A longer silence is a hung agent, not a slow model.
pub const MAX_ENGINE_STALL_MINUTES: u64 = 7 * 24 * 60;

pub const DEFAULT_MAX_CONCURRENT_TURNS: usize = 32;

impl SchedulerSection {
    pub fn concurrency(&self) -> Result<usize, String> {
        match self.max_concurrent_turns {
            Some(0) => Err("scheduler.max_concurrent_turns must be positive".into()),
            Some(value) => Ok(value),
            None => Ok(DEFAULT_MAX_CONCURRENT_TURNS),
        }
    }

    /// The configured bound. `None` means no automatic check-in; a
    /// value that cannot mean what it says is reported, not silently dropped.
    pub fn wait_timeout(&self) -> Result<Option<std::time::Duration>, String> {
        let Some(minutes) = self.wait_timeout_minutes else {
            return Ok(None);
        };
        if minutes == 0 || minutes > MAX_WAIT_TIMEOUT_MINUTES {
            return Err(format!(
                "wait_timeout_minutes must be between 1 and {MAX_WAIT_TIMEOUT_MINUTES}; \
                 remove it for the default"
            ));
        }
        Ok(Some(std::time::Duration::from_secs(minutes * 60)))
    }

    /// The configured engine-turn cap. `None` means unlimited. Zero is
    /// rejected: it would admit no engine turn at all, which is a mistake
    /// rather than a policy - an operator who wants none removes the model.
    pub fn engine_concurrency(&self) -> Result<Option<usize>, String> {
        match self.max_concurrent_engine_turns {
            Some(0) => Err("scheduler.max_concurrent_engine_turns must be positive; \
                            remove it for no engine cap"
                .into()),
            other => Ok(other),
        }
    }

    /// The configured per-run spend cap in dollars. `None` means no budget.
    /// A value that is not a positive, finite number of dollars cannot mean
    /// what it says, so it is reported rather than silently dropped.
    pub fn run_budget(&self) -> Result<Option<f64>, String> {
        let Some(usd) = self.run_budget_usd else {
            return Ok(None);
        };
        if !usd.is_finite() || usd <= 0.0 {
            return Err(format!(
                "scheduler.run_budget_usd must be a positive number of dollars (got {usd}); \
                 remove it for no run budget"
            ));
        }
        Ok(Some(usd))
    }

    /// The configured hard stop. Non-positive values cannot mean what they say
    /// (a zero stop would interrupt every run at once), so they are reported.
    pub fn run_hard_stop(&self) -> Result<aether_core::budget::HardStop, String> {
        if let Some(usd) = self.run_hard_stop_usd
            && (!usd.is_finite() || usd <= 0.0)
        {
            return Err(format!(
                "scheduler.run_hard_stop_usd must be a positive number of dollars (got {usd}); \
                 remove it for no hard stop"
            ));
        }
        if self.run_hard_stop_tokens == Some(0) {
            return Err(
                "scheduler.run_hard_stop_tokens must be positive; remove it for no token stop"
                    .into(),
            );
        }
        Ok(aether_core::budget::HardStop {
            usd: self.run_hard_stop_usd,
            tokens: self.run_hard_stop_tokens,
        })
    }

    /// The configured grace. `None` means the runtime default applies. Zero
    /// is rejected: it would kill the engine the instant a harness tool
    /// landed, before it could report what it had already done.
    pub fn engine_park_grace(&self) -> Result<Option<std::time::Duration>, String> {
        let Some(seconds) = self.engine_park_grace_secs else {
            return Ok(None);
        };
        if seconds == 0 || seconds > MAX_ENGINE_PARK_GRACE_SECS {
            return Err(format!(
                "engine_park_grace_secs must be between 1 and {MAX_ENGINE_PARK_GRACE_SECS}; \
                 remove it for the default"
            ));
        }
        Ok(Some(std::time::Duration::from_secs(seconds)))
    }

    /// The configured transient-retry window. `None` means the runtime
    /// default applies; zero fails an engine turn on its first transient error.
    pub fn engine_transient_retry(&self) -> Result<Option<std::time::Duration>, String> {
        let Some(minutes) = self.engine_transient_retry_minutes else {
            return Ok(None);
        };
        if minutes > MAX_ENGINE_TRANSIENT_RETRY_MINUTES {
            return Err(format!(
                "engine_transient_retry_minutes must be at most \
                 {MAX_ENGINE_TRANSIENT_RETRY_MINUTES}; remove it for the default"
            ));
        }
        Ok(Some(std::time::Duration::from_secs(minutes * 60)))
    }

    /// The configured stall timeout. `None` means the runtime default
    /// applies; a zero duration turns the watchdog off.
    pub fn engine_stall_timeout(&self) -> Result<Option<std::time::Duration>, String> {
        let Some(minutes) = self.engine_stall_minutes else {
            return Ok(None);
        };
        if minutes > MAX_ENGINE_STALL_MINUTES {
            return Err(format!(
                "engine_stall_minutes must be at most {MAX_ENGINE_STALL_MINUTES}; \
                 remove it for the default"
            ));
        }
        Ok(Some(std::time::Duration::from_secs(minutes * 60)))
    }

    /// The free space new turns wait for. `None` means they never wait.
    pub fn disk_floor(&self) -> Result<Option<DiskFloor>, String> {
        match self.min_free_disk_mib {
            None => Ok(Some(DiskFloor::Default)),
            Some(0) => Ok(None),
            Some(mib) => match mib.checked_mul(1 << 20) {
                Some(bytes) => Ok(Some(DiskFloor::Bytes(bytes))),
                None => Err(format!(
                    "scheduler.min_free_disk_mib is too large (got {mib}); \
                     remove it for the default"
                )),
            },
        }
    }
}
