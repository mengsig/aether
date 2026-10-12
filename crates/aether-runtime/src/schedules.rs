//! Durable timers enqueue triggers, or start runs, through the same commit
//! path as messages.

use std::collections::BTreeSet;

use aether_core::agent::Agent;
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::{AgentId, ScheduleId};
use aether_core::idempotency::{Submission, SubmissionOutcome};
use aether_core::schedule::{
    BusyPolicy, Recurrence, Schedule, ScheduleMode, ScheduleStatus, ScheduleTarget, SpawnSpec,
};
use aether_core::time::{Timestamp, now};
use aether_store::{Commit, Mutation};
use chrono::Datelike;

use crate::commands::Preflight;
use crate::submission::{created_work_gone, wrong_shape};
use crate::{Result, Runtime, RuntimeError};

pub use aether_core::schedule::{CreateScheduleSpec, ScheduleAction};

mod execution;
use execution::spawn_request;
pub(crate) use execution::tick;

/// Why `action` cannot apply to a schedule that has already ended, worded
/// for the operator who asked.
fn refused_change(id: ScheduleId, action: ScheduleAction, status: &ScheduleStatus) -> String {
    let verb = match action {
        ScheduleAction::Pause => "pause",
        ScheduleAction::Resume => "resume",
        ScheduleAction::Cancel => "cancel",
    };
    let stamp = |at: &Timestamp| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let why = match status {
        ScheduleStatus::Active { .. } => "it is active".to_owned(),
        ScheduleStatus::Paused { .. } => "it is paused".to_owned(),
        ScheduleStatus::Cancelled { at } => format!("it was cancelled at {}", stamp(at)),
        ScheduleStatus::Exhausted { at } => {
            format!("it ran its last occurrence at {}", stamp(at))
        }
        ScheduleStatus::Failed { at, error } => {
            format!("it failed at {}: {error}", stamp(at))
        }
    };
    format!("cannot {verb} schedule {id}: {why}")
}

impl Runtime {
    pub fn create_schedule(&self, spec: CreateScheduleSpec, by: Actor) -> Result<Schedule> {
        self.create_schedule_keyed(spec, by, None)
    }

    /// Creation a client can retry: under `submission`'s key, a second
    /// identical request returns the job the first one created rather than a
    /// second timer firing on the same recurrence.
    pub fn create_schedule_keyed(
        &self,
        spec: CreateScheduleSpec,
        by: Actor,
        submission: Option<Submission>,
    ) -> Result<Schedule> {
        if let Some(outcome) = self.recorded_submission(submission.as_ref())? {
            return self.replayed_schedule(outcome);
        }
        let name = spec.name.trim();
        if name.is_empty() || name.len() > 256 {
            return Err(RuntimeError::Invalid(
                "schedule name must contain 1–256 bytes".into(),
            ));
        }
        // The name is echoed into a spawned run's one-line objective suffix
        // and into terminal listings.
        if name.chars().any(char::is_control) {
            return Err(RuntimeError::Invalid(
                "schedule name must not contain control characters".into(),
            ));
        }
        let at = now();
        let next_due = next_occurrence(&spec.recurrence, at)?;
        let target = match spec.target {
            ScheduleTarget::Agent(id) => ScheduleTarget::Agent(id),
            ScheduleTarget::Spawn(template) => {
                match self.checked_template(template, &by, spec.busy) {
                    Ok(template) => ScheduleTarget::Spawn(template),
                    // A retry racing its landed original replays it, not the error.
                    Err(error) => match self.recorded_submission(submission.as_ref())? {
                        Some(outcome) => return self.replayed_schedule(outcome),
                        None => return Err(error),
                    },
                }
            }
        };
        let hold = self.hold();
        // Before the target is resolved: a retry must not be refused because
        // the agent the first submission scheduled for has since finished.
        if let Some(outcome) = self.replayed_submission(&hold, submission.as_ref())? {
            drop(hold);
            return self.replayed_schedule(outcome);
        }
        let mode = match target {
            ScheduleTarget::Agent(id) => {
                let target = self.schedule_target(id, &by)?;
                ScheduleMode::Deliver {
                    target: target.id,
                    run: target.run,
                    last_inbox_seq: None,
                }
            }
            ScheduleTarget::Spawn(template) => ScheduleMode::Spawn {
                template,
                last_spawned: None,
            },
        };
        let schedule = Schedule {
            id: ScheduleId::new(),
            name: name.into(),
            owner: by.clone(),
            mode,
            recurrence: spec.recurrence,
            missed: spec.missed,
            busy: spec.busy,
            payload: spec.payload,
            status: ScheduleStatus::Active { next_due },
            max_deliveries: spec.max_deliveries,
            deliveries: 0,
            last: None,
            created_at: at,
            updated_at: at,
        };
        let mut commit = Commit::new();
        record_schedule(&mut commit, &schedule, "created", by);
        crate::submission::record_receipt(&mut commit, submission.as_ref(), &schedule)?;
        self.apply_held(hold, commit)?;
        Ok(schedule)
    }

    fn replayed_schedule(&self, outcome: SubmissionOutcome) -> Result<Schedule> {
        if let SubmissionOutcome::Receipt { value } = outcome {
            return serde_json::from_value(value)
                .map_err(|e| RuntimeError::Internal(format!("decode schedule receipt: {e}")));
        }
        let SubmissionOutcome::Schedule { schedule } = outcome else {
            return Err(wrong_shape(&outcome, "schedule"));
        };
        self.store
            .schedule(schedule)?
            .ok_or_else(|| created_work_gone("schedule", &schedule))
    }

    /// The template every occurrence spawns from, model frozen. Operators
    /// only: each run gets what `create_agent` would grant, not an agent's.
    fn checked_template(
        &self,
        mut template: SpawnSpec,
        by: &Actor,
        busy: BusyPolicy,
    ) -> Result<SpawnSpec> {
        let refuse = |why: &str| Err(RuntimeError::Invalid(why.into()));
        if !matches!(by, Actor::User { .. } | Actor::Client { .. }) {
            return refuse(
                "only an operator can create a spawn schedule; agents schedule deliveries to \
                 themselves or their descendants",
            );
        }
        if busy != BusyPolicy::Skip {
            return refuse(
                "busy=queue_one applies to delivery schedules; a spawn schedule skips while \
                 its previous run is live",
            );
        }
        if template.role.trim().is_empty() || template.objective.trim().is_empty() {
            return refuse("a spawn template needs a role and an objective");
        }
        if let Some(config) = &template.config {
            if config.deadline.is_some() {
                return refuse(
                    "a spawn template cannot set an absolute deadline: it would pass after \
                     the first run; use max_turns or a budget",
                );
            }
            if config.long_lived {
                return refuse(
                    "a spawn template cannot be long-lived: its first run would never end, \
                     so every later occurrence would be skipped",
                );
            }
        }
        let model = template
            .model
            .take()
            .or_else(|| self.default_model())
            .ok_or_else(|| {
                RuntimeError::Invalid("no model configured; sign in from your client".into())
            })?;
        template.model = Some(model);
        // Refuse now what every occurrence would fail on. Nothing is written.
        let objective = template.objective.clone();
        self.prepare_agent(
            spawn_request(&template, objective),
            by,
            Preflight::default(),
        )?;
        Ok(template)
    }

    pub fn update_schedule(
        &self,
        id: ScheduleId,
        action: ScheduleAction,
        by: Actor,
    ) -> Result<Schedule> {
        self.update_schedule_keyed(id, action, by, None)
    }

    pub fn update_schedule_keyed(
        &self,
        id: ScheduleId,
        action: ScheduleAction,
        by: Actor,
        submission: Option<aether_core::idempotency::Submission>,
    ) -> Result<Schedule> {
        let hold = self.hold();
        if let Some(receipt) = self.replay_receipt(&hold, submission.as_ref())? {
            return Ok(receipt);
        }
        let mut job = self
            .store
            .schedule(id)?
            .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?;
        if !matches!(by, Actor::User { .. } | Actor::Client { .. }) && by != job.owner {
            return Err(RuntimeError::Invalid(format!(
                "only the owner or an operator can change schedule {id}"
            )));
        }
        let at = now();
        let (status, verb) = match (action, &job.status) {
            (ScheduleAction::Pause, ScheduleStatus::Active { next_due }) => (
                ScheduleStatus::Paused {
                    next_due: *next_due,
                },
                "paused",
            ),
            (ScheduleAction::Pause, ScheduleStatus::Paused { .. })
            | (ScheduleAction::Resume, ScheduleStatus::Active { .. })
            | (ScheduleAction::Cancel, ScheduleStatus::Cancelled { .. }) => {
                self.commit_receipt(hold, Commit::new(), submission.as_ref(), &job)?;
                return Ok(job);
            }
            (ScheduleAction::Resume, ScheduleStatus::Paused { next_due }) => {
                self.check_delivery_target(&job)?;
                (
                    ScheduleStatus::Active {
                        next_due: *next_due,
                    },
                    "resumed",
                )
            }
            (ScheduleAction::Resume, ScheduleStatus::Failed { .. }) => {
                self.check_delivery_target(&job)?;
                (
                    ScheduleStatus::Active {
                        next_due: next_occurrence(&job.recurrence, at)?,
                    },
                    "resumed",
                )
            }
            (ScheduleAction::Cancel, _) => (ScheduleStatus::Cancelled { at }, "cancelled"),
            (action, status) => {
                return Err(RuntimeError::Invalid(refused_change(id, action, status)));
            }
        };
        job.status = status;
        job.updated_at = at;
        let mut commit = Commit::new();
        record_schedule(&mut commit, &job, verb, by);
        self.commit_receipt(hold, commit, submission.as_ref(), &job)?;
        Ok(job)
    }

    /// A spawn schedule has no target to recheck; its next occurrence
    /// revalidates the template and fails the job again if it still cannot run.
    fn check_delivery_target(&self, job: &Schedule) -> Result<()> {
        if let Some(target) = job.target() {
            self.schedule_target(target, &job.owner)?;
        }
        Ok(())
    }

    fn schedule_target(&self, id: AgentId, owner: &Actor) -> Result<Agent> {
        let target = self
            .store
            .agent(id)?
            .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?;
        if target.status.is_terminal() {
            return Err(RuntimeError::Invalid(format!(
                "schedule target {id} is {}",
                target.status.kind()
            )));
        }
        match owner {
            Actor::User { .. } | Actor::Client { .. } => return Ok(target),
            Actor::Agent { id: creator } => {
                let parent = self
                    .store
                    .agent(*creator)?
                    .ok_or_else(|| RuntimeError::NotFound(creator.to_string()))?;
                if !self.scope_within_ancestor(&target, &parent, &parent.scope)?
                    || !target
                        .config
                        .approval
                        .is_narrowing_of(&parent.config.approval)
                {
                    return Err(RuntimeError::Invalid(
                        "schedule target would widen its creator's authority".into(),
                    ));
                }
                let mut cursor = Some(target.id);
                let mut seen = BTreeSet::new();
                while let Some(id) = cursor {
                    if id == *creator {
                        return Ok(target);
                    }
                    if !seen.insert(id) {
                        return Err(RuntimeError::Invalid(
                            "cycle in schedule target ancestry".into(),
                        ));
                    }
                    cursor = self
                        .store
                        .agent(id)?
                        .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?
                        .parent;
                }
            }
            _ => {}
        }
        Err(RuntimeError::Invalid(
            "agents may schedule only themselves or their descendants".into(),
        ))
    }
}

fn next_occurrence(recurrence: &Recurrence, at: Timestamp) -> Result<Timestamp> {
    let next = recurrence
        .next_after(at)
        .map_err(|error| RuntimeError::Invalid(error.to_string()))?;
    check_calendar_range(next)?;
    Ok(next)
}

fn check_calendar_range(at: Timestamp) -> Result<()> {
    if !(1..=9999).contains(&at.year()) {
        return Err(RuntimeError::Invalid(
            "schedule dates must be within years 1–9999".into(),
        ));
    }
    Ok(())
}

/// Delivery schedules file their events under the target and its run.
fn about(event: EventDraft, job: &Schedule) -> EventDraft {
    match &job.mode {
        ScheduleMode::Deliver { target, run, .. } => event.agent(*target).run(*run),
        ScheduleMode::Spawn { .. } => event,
    }
}

fn record_schedule(commit: &mut Commit, job: &Schedule, verb: &str, by: Actor) {
    commit.push_event(about(
        EventDraft::new(
            by,
            EventKind::Custom {
                name: format!("schedule.{verb}"),
                payload: serde_json::json!(job),
            },
        ),
        job,
    ));
    commit.push_mutation(Mutation::UpsertSchedule(job.clone()));
}

#[cfg(test)]
mod tests;
