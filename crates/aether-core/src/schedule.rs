//! Recurrence calculations, independent of storage and trigger delivery.

use std::num::NonZeroU64;
use std::str::FromStr;

use chrono::{Datelike, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use crate::agent::{AgentConfig, ApprovalPolicy, Scope};
use crate::event::Actor;
use crate::id::{AgentId, InboxItemId, RunId, ScheduleId, TaskId};
use crate::model::ModelRef;
use crate::time::Timestamp;

/// The shortest interval a new schedule may recur at: each occurrence can be
/// a paid turn, and cron recurs no faster than once a minute either.
pub const MIN_INTERVAL_SECONDS: u64 = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Recurrence {
    Interval {
        seconds: NonZeroU64,
    },
    /// Five fields: minute, hour, day of month, month, day of week.
    Cron {
        expression: String,
        timezone: String,
    },
}

/// When several occurrences elapsed between scheduler ticks, coalescing
/// delivers one trigger; skipping advances the clock without delivering one.
/// A tick that is late by less than one recurrence still delivers normally.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MissedRunPolicy {
    #[default]
    Coalesce,
    Skip,
}

/// One bounded decision for a due job, regardless of the length of downtime.
/// Persist the next due time together with the delivery or skip record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueOccurrence {
    pub due: Timestamp,
    pub next_due: Timestamp,
    pub deliver: bool,
}

/// What an occurrence does when the target is busy. Either way at most one
/// trigger from a schedule is ever pending in the target's inbox.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum BusyPolicy {
    /// Skip while the target is running or the previous trigger is pending.
    #[default]
    Skip,
    /// Deliver while the target is running, once the previous trigger has
    /// been consumed, so the next turn picks it up.
    QueueOne,
}

impl BusyPolicy {
    fn is_skip(&self) -> bool {
        *self == Self::Skip
    }
}

/// Root agent + task each spawn occurrence creates, in a new run. Fields as on
/// `create_agent`; no parent or existing run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SpawnSpec {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Each run's task. The occurrence's due time is appended to it.
    pub objective: String,
    /// Resolved when the schedule is created, so every run uses the model
    /// shown by `get_schedule` rather than whatever the default is later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<ModelRef>,
    /// Absent: the daemon's default root scope when each run starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
    /// Budgets bound each run. Absolute deadline (stale by run two) and
    /// `long_lived` (keeps run one live, skipping the rest) are refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<AgentConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalPolicy>,
}

/// What a new schedule's occurrences do.
// Records, built once and serialized; boxing would buy only an allocation.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleTarget {
    /// Deliver the payload to this existing agent's inbox.
    Agent(AgentId),
    /// Start a fresh root run from this template.
    Spawn(SpawnSpec),
}

/// On the wire `target` and `spawn` are sibling fields, exactly one set, so
/// a deliver request keeps the shape older clients send.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(try_from = "CreateScheduleWire", into = "CreateScheduleWire")]
pub struct CreateScheduleSpec {
    pub name: String,
    pub target: ScheduleTarget,
    pub recurrence: Recurrence,
    pub missed: MissedRunPolicy,
    pub busy: BusyPolicy,
    pub payload: serde_json::Value,
    pub max_deliveries: Option<NonZeroU64>,
}

/// The wire shape of [`CreateScheduleSpec`]; use that type instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[doc(hidden)]
pub struct CreateScheduleWire {
    pub name: String,
    /// The agent that receives each trigger. Exactly one of `target` and
    /// `spawn`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<AgentId>,
    /// Start a new root run on each occurrence instead of delivering to an
    /// agent. Operators and clients only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn: Option<SpawnSpec>,
    pub recurrence: Recurrence,
    #[serde(default)]
    pub missed: MissedRunPolicy,
    /// Delivery schedules only; spawn schedules skip while a run is live.
    #[serde(default, skip_serializing_if = "BusyPolicy::is_skip")]
    pub busy: BusyPolicy,
    /// Trigger payload; spawn schedules append non-null payloads to each task.
    /// Reaches model and event log verbatim: no secrets.
    #[serde(default)]
    pub payload: serde_json::Value,
    #[serde(default)]
    pub max_deliveries: Option<NonZeroU64>,
}

impl TryFrom<CreateScheduleWire> for CreateScheduleSpec {
    type Error = String;

    fn try_from(wire: CreateScheduleWire) -> Result<Self, String> {
        let target = match (wire.target, wire.spawn) {
            (Some(agent), None) => ScheduleTarget::Agent(agent),
            (None, Some(template)) => ScheduleTarget::Spawn(template),
            _ => return Err("a schedule names exactly one of `target` or `spawn`".into()),
        };
        Ok(Self {
            name: wire.name,
            target,
            recurrence: wire.recurrence,
            missed: wire.missed,
            busy: wire.busy,
            payload: wire.payload,
            max_deliveries: wire.max_deliveries,
        })
    }
}

impl From<CreateScheduleSpec> for CreateScheduleWire {
    fn from(spec: CreateScheduleSpec) -> Self {
        let (target, spawn) = match spec.target {
            ScheduleTarget::Agent(agent) => (Some(agent), None),
            ScheduleTarget::Spawn(template) => (None, Some(template)),
        };
        Self {
            name: spec.name,
            target,
            spawn,
            recurrence: spec.recurrence,
            missed: spec.missed,
            busy: spec.busy,
            payload: spec.payload,
            max_deliveries: spec.max_deliveries,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleAction {
    Pause,
    Resume,
    Cancel,
}

/// Timers enqueue work, never authority: targets keep their scope and budgets;
/// spawned runs get only what an operator could grant via `create_agent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(try_from = "ScheduleWire", into = "ScheduleWire")]
pub struct Schedule {
    pub id: ScheduleId,
    pub name: String,
    pub owner: Actor,
    pub mode: ScheduleMode,
    pub recurrence: Recurrence,
    pub missed: MissedRunPolicy,
    pub busy: BusyPolicy,
    pub payload: serde_json::Value,
    pub status: ScheduleStatus,
    /// Optional lifetime limit on deliveries or spawned runs, independent of
    /// any agent's budgets.
    pub max_deliveries: Option<NonZeroU64>,
    /// Occurrences that delivered a trigger or spawned a run.
    pub deliveries: u64,
    /// Mirrors the newest history entry; written in the same commit.
    pub last: Option<ScheduleOccurrence>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

// Records, built once and serialized; boxing would buy only an allocation.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleMode {
    Deliver {
        target: AgentId,
        run: RunId,
        /// Bounds overlap: a timer waits until its last trigger has been
        /// consumed before delivering another.
        last_inbox_seq: Option<u64>,
    },
    Spawn {
        template: SpawnSpec,
        /// Bounds overlap: no new run starts while this one is live.
        last_spawned: Option<AgentId>,
    },
}

impl Schedule {
    /// The agent a delivery schedule delivers to; `None` for spawn schedules.
    pub fn target(&self) -> Option<AgentId> {
        match &self.mode {
            ScheduleMode::Deliver { target, .. } => Some(*target),
            ScheduleMode::Spawn { .. } => None,
        }
    }

    /// The run a delivery schedule belongs to; `None` for spawn schedules.
    pub fn run(&self) -> Option<RunId> {
        match &self.mode {
            ScheduleMode::Deliver { run, .. } => Some(*run),
            ScheduleMode::Spawn { .. } => None,
        }
    }
}

/// Wire/storage shape of [`Schedule`]. Delivery schedules keep protocol 8's
/// fields and order; `busy` and `last` appear only when set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[doc(hidden)]
pub struct ScheduleWire {
    pub id: ScheduleId,
    pub name: String,
    pub owner: Actor,
    /// Delivery schedules: the agent that receives each trigger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<AgentId>,
    /// Delivery schedules: the target's run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunId>,
    /// Spawn schedules: the template each occurrence starts a run from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn: Option<SpawnSpec>,
    /// Spawn schedules: the root of the newest spawned run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_spawned: Option<AgentId>,
    pub recurrence: Recurrence,
    pub missed: MissedRunPolicy,
    #[serde(default, skip_serializing_if = "BusyPolicy::is_skip")]
    pub busy: BusyPolicy,
    pub payload: serde_json::Value,
    pub status: ScheduleStatus,
    pub max_deliveries: Option<NonZeroU64>,
    pub deliveries: u64,
    /// Delivery schedules: the inbox sequence of the newest trigger.
    #[serde(default)]
    pub last_inbox_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<ScheduleOccurrence>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl TryFrom<ScheduleWire> for Schedule {
    type Error = String;

    fn try_from(wire: ScheduleWire) -> Result<Self, String> {
        let mode = match (wire.target, wire.run, wire.spawn) {
            (Some(target), Some(run), None) if wire.last_spawned.is_none() => {
                ScheduleMode::Deliver {
                    target,
                    run,
                    last_inbox_seq: wire.last_inbox_seq,
                }
            }
            (None, None, Some(template)) if wire.last_inbox_seq.is_none() => ScheduleMode::Spawn {
                template,
                last_spawned: wire.last_spawned,
            },
            _ => {
                return Err(
                    "a schedule has either `target` and `run` or a `spawn` template".into(),
                );
            }
        };
        Ok(Self {
            id: wire.id,
            name: wire.name,
            owner: wire.owner,
            mode,
            recurrence: wire.recurrence,
            missed: wire.missed,
            busy: wire.busy,
            payload: wire.payload,
            status: wire.status,
            max_deliveries: wire.max_deliveries,
            deliveries: wire.deliveries,
            last: wire.last,
            created_at: wire.created_at,
            updated_at: wire.updated_at,
        })
    }
}

impl From<Schedule> for ScheduleWire {
    fn from(job: Schedule) -> Self {
        let (target, run, last_inbox_seq, spawn, last_spawned) = match job.mode {
            ScheduleMode::Deliver {
                target,
                run,
                last_inbox_seq,
            } => (Some(target), Some(run), last_inbox_seq, None, None),
            ScheduleMode::Spawn {
                template,
                last_spawned,
            } => (None, None, None, Some(template), last_spawned),
        };
        Self {
            id: job.id,
            name: job.name,
            owner: job.owner,
            target,
            run,
            spawn,
            last_spawned,
            recurrence: job.recurrence,
            missed: job.missed,
            busy: job.busy,
            payload: job.payload,
            status: job.status,
            max_deliveries: job.max_deliveries,
            deliveries: job.deliveries,
            last_inbox_seq,
            last: job.last,
            created_at: job.created_at,
            updated_at: job.updated_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ScheduleStatus {
    Active { next_due: Timestamp },
    Paused { next_due: Timestamp },
    Cancelled { at: Timestamp },
    Exhausted { at: Timestamp },
    Failed { at: Timestamp, error: String },
}

impl ScheduleStatus {
    pub fn next_due(&self) -> Option<Timestamp> {
        match self {
            Self::Active { next_due } => Some(*next_due),
            _ => None,
        }
    }
}

/// History describes delivery or spawning, never claims that a model
/// completed the requested work. The ids link to the execution trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ScheduleOccurrence {
    pub schedule: ScheduleId,
    pub due: Timestamp,
    pub recorded_at: Timestamp,
    pub outcome: ScheduleOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ScheduleOutcome {
    Delivered {
        inbox: InboxItemId,
    },
    /// A spawn schedule started a run: its root agent and task.
    Spawned {
        agent: AgentId,
        task: TaskId,
        run: RunId,
    },
    Skipped {
        reason: String,
    },
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecurrenceError {
    #[error("interval seconds `{0}` exceed the supported duration range")]
    IntervalRange(u64),
    #[error("cron expression `{expression}` is invalid: {reason}")]
    Cron { expression: String, reason: String },
    #[error("unknown timezone `{0}`; use an IANA name such as Europe/Copenhagen or UTC")]
    Timezone(String),
    #[error("the next occurrence is outside the supported timestamp range")]
    TimestampRange,
}

impl Recurrence {
    /// Strictly after the supplied instant, never at that same instant.
    pub fn next_after(&self, after: Timestamp) -> Result<Timestamp, RecurrenceError> {
        match self {
            Self::Interval { seconds } => add_seconds(after, seconds.get()),
            Self::Cron {
                expression,
                timezone,
            } => {
                // Keep the library's calendar search away from Chrono's
                // representational edges, where calendar arithmetic may panic.
                if !(1..=9998).contains(&after.year()) {
                    return Err(RecurrenceError::TimestampRange);
                }
                if expression.len() > 256 || expression.split_whitespace().count() != 5 {
                    return Err(RecurrenceError::Cron {
                        expression: expression.clone(),
                        reason: "expected five fields (minute hour day month weekday), at most 256 bytes".into(),
                    });
                }
                let zone = chrono_tz::Tz::from_str(timezone)
                    .map_err(|_| RecurrenceError::Timezone(timezone.clone()))?;
                let cron =
                    croner::Cron::from_str(expression).map_err(|error| RecurrenceError::Cron {
                        expression: expression.clone(),
                        reason: error.to_string(),
                    })?;
                let next = cron
                    .find_next_occurrence(&after.with_timezone(&zone), false)
                    .map_err(|error| RecurrenceError::Cron {
                        expression: expression.clone(),
                        reason: error.to_string(),
                    })?
                    .with_timezone(&Utc);
                if next <= after || next.year() > 9999 {
                    return Err(RecurrenceError::TimestampRange);
                }
                Ok(next)
            }
        }
    }

    /// Plan a single delivery or skip. No work is returned before the due
    /// time, and missed occurrences never produce an unbounded replay burst.
    pub fn plan_due(
        &self,
        due: Timestamp,
        now: Timestamp,
        missed: MissedRunPolicy,
    ) -> Result<Option<DueOccurrence>, RecurrenceError> {
        if due > now {
            return Ok(None);
        }
        let next_due = self.advance_past(due, now)?;
        let deliver = missed == MissedRunPolicy::Coalesce || self.next_after(due)? > now;
        Ok(Some(DueOccurrence {
            due,
            next_due,
            deliver,
        }))
    }

    /// Advance an overdue occurrence beyond now without replaying each missed
    /// interval. The original interval cadence survives downtime and late ticks.
    pub fn advance_past(
        &self,
        due: Timestamp,
        now: Timestamp,
    ) -> Result<Timestamp, RecurrenceError> {
        if due > now {
            return Ok(due);
        }
        match self {
            Self::Cron { .. } => self.next_after(now),
            Self::Interval { seconds } => {
                let elapsed = (now - due).num_seconds().unsigned_abs();
                let steps = elapsed / seconds.get() + 1;
                let offset = seconds
                    .get()
                    .checked_mul(steps)
                    .ok_or(RecurrenceError::TimestampRange)?;
                add_seconds(due, offset)
            }
        }
    }
}

fn add_seconds(at: Timestamp, seconds: u64) -> Result<Timestamp, RecurrenceError> {
    let duration = i64::try_from(seconds)
        .ok()
        .and_then(TimeDelta::try_seconds)
        .ok_or(RecurrenceError::IntervalRange(seconds))?;
    at.checked_add_signed(duration)
        .ok_or(RecurrenceError::TimestampRange)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests;
