//! Host evidence is append-only; counted verdicts move recall ranking within bounds.

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::{InvocationId, MemoryFeedbackId, MemoryId};
use aether_core::idempotency::{Submission, SubmissionOutcome};
use aether_core::memory_feedback::{
    MAX_FEEDBACK_REASON_CHARS, MEMORY_FEEDBACK_LIMIT, MemoryFeedback, MemoryFeedbackList,
    MemoryFeedbackSpec,
};
use aether_core::snapshot::SnapshotMemories;
use aether_core::task::Task;
use aether_core::time::now;
use aether_store::{Commit, EventFilter, Mutation};

use crate::{Result, Runtime, RuntimeError};

impl Runtime {
    pub fn record_memory_feedback(
        &self,
        spec: MemoryFeedbackSpec,
        by: Actor,
    ) -> Result<MemoryFeedback> {
        self.record_memory_feedback_keyed(spec, by, None)
    }

    pub fn record_memory_feedback_keyed(
        &self,
        spec: MemoryFeedbackSpec,
        by: Actor,
        submission: Option<Submission>,
    ) -> Result<MemoryFeedback> {
        require_operator(&by)?;
        if let Some(outcome) = self.recorded_submission(submission.as_ref())? {
            let SubmissionOutcome::Receipt { value } = outcome else {
                return Err(crate::submission::wrong_shape(&outcome, "receipt"));
            };
            return serde_json::from_value(value).map_err(|error| {
                RuntimeError::Internal(format!("decode feedback receipt: {error}"))
            });
        }
        validate_reason(&spec.reason)?;
        self.check_feedback_context(&spec)?;
        let hold = self.hold();
        if let Some(receipt) = self.replay_receipt(&hold, submission.as_ref())? {
            return Ok(receipt);
        }
        self.store
            .memory(spec.memory)?
            .ok_or_else(|| RuntimeError::NotFound(format!("memory {}", spec.memory)))?;
        let task = self
            .store
            .task(spec.task)?
            .ok_or_else(|| RuntimeError::NotFound(format!("task {}", spec.task)))?;
        debug_assert_eq!(task.id, spec.task);
        let (record, commit) = feedback_commit(spec, by, task.run)?;
        self.commit_receipt(hold, commit, submission.as_ref(), &record)?;
        Ok(record)
    }

    fn check_feedback_context(&self, spec: &MemoryFeedbackSpec) -> Result<()> {
        self.store
            .memory(spec.memory)?
            .ok_or_else(|| RuntimeError::NotFound(format!("memory {}", spec.memory)))?;
        let task = self
            .store
            .task(spec.task)?
            .ok_or_else(|| RuntimeError::NotFound(format!("task {}", spec.task)))?;
        let context = spec
            .invocation
            .map(|id| self.feedback_invocation_context(id, &task))
            .transpose()?;
        if context
            .as_ref()
            .is_some_and(|entry| entry.memories.contains(&spec.memory))
        {
            return Ok(());
        }
        let through = context.map(|entry| entry.at);
        if !self
            .store
            .memory_was_in_task_context(spec.memory, spec.task, through)?
        {
            return Err(RuntimeError::Invalid(format!(
                "memory {} was not in context for task {}",
                spec.memory, spec.task
            )));
        }
        Ok(())
    }

    fn feedback_invocation_context(
        &self,
        id: InvocationId,
        task: &Task,
    ) -> Result<SnapshotMemories> {
        let owner = task
            .owner
            .ok_or_else(|| RuntimeError::NotFound(format!("invocation {id}")))?;
        let snapshot = match self.store.invocation(id)? {
            Some(call) => call.snapshot_id,
            None => self
                .failed_call_snapshot(id, EventFilter::own_events(owner))?
                .ok_or_else(|| RuntimeError::NotFound(format!("invocation {id}")))?,
        };
        let mut found = self.store.snapshot_memories(&[snapshot])?;
        let entry = found.pop().ok_or_else(|| {
            RuntimeError::NotFound(format!("snapshot {snapshot} of invocation {id}"))
        })?;
        debug_assert!(found.is_empty());
        debug_assert_eq!(entry.snapshot, snapshot);
        if entry.task != Some(task.id) {
            return Err(RuntimeError::Invalid(format!(
                "invocation {id} is not a call of task {}",
                task.id
            )));
        }
        Ok(entry)
    }

    pub fn retract_memory_feedback(
        &self,
        id: MemoryFeedbackId,
        reason: String,
        by: Actor,
    ) -> Result<MemoryFeedback> {
        require_operator(&by)?;
        validate_reason(&reason)?;
        let hold = self.hold();
        let mut record = self
            .store
            .memory_feedback_record(id)?
            .ok_or_else(|| RuntimeError::NotFound(format!("feedback {id}")))?;
        if record.retracted_at.is_some() {
            return Ok(record);
        }
        debug_assert_eq!(record.id, id);
        debug_assert!(record.retraction_reason.is_none());
        let at = now();
        record.retracted_at = Some(at);
        record.retraction_reason = Some(reason.clone());
        let mut event = EventDraft::new(
            by,
            EventKind::Custom {
                name: "memory.feedback_retracted".into(),
                payload: serde_json::json!({"feedback":id, "memory":record.spec.memory, "reason":reason}),
            },
        );
        if let Some(task) = self.store.task(record.spec.task)? {
            event.task_id = Some(task.id);
            event.run = Some(task.run);
        }
        self.commit_receipt(
            hold,
            Commit::new()
                .event(event)
                .mutate(Mutation::RetractMemoryFeedback { id, at, reason }),
            None,
            &record,
        )?;
        Ok(record)
    }

    pub fn memory_feedback(
        &self,
        memory: MemoryId,
        limit: Option<usize>,
        by: &Actor,
    ) -> Result<MemoryFeedbackList> {
        require_operator(by)?;
        let limit = limit.unwrap_or(MEMORY_FEEDBACK_LIMIT);
        if !(1..=MEMORY_FEEDBACK_LIMIT).contains(&limit) {
            return Err(RuntimeError::Invalid(format!(
                "limit must be between 1 and {MEMORY_FEEDBACK_LIMIT}"
            )));
        }
        self.store
            .memory(memory)?
            .ok_or_else(|| RuntimeError::NotFound(format!("memory {memory}")))?;
        let mut records = self.store.memory_feedback(memory, limit + 1)?;
        let truncated = records.len() > limit;
        records.truncate(limit);
        let tally = self.store.memory_feedback_tallies(&[memory])?;
        debug_assert!(records.len() <= limit);
        debug_assert!(records.iter().all(|record| record.spec.memory == memory));
        debug_assert!(tally.keys().all(|id| *id == memory));
        let ranking = tally.get(&memory).copied().unwrap_or_default().into();
        Ok(MemoryFeedbackList {
            records,
            truncated,
            ranking,
        })
    }
}

fn require_operator(by: &Actor) -> Result<()> {
    if !matches!(by, Actor::User { .. }) {
        return Err(RuntimeError::Invalid(
            "only an operator can access memory feedback".into(),
        ));
    }
    Ok(())
}

fn validate_reason(reason: &str) -> Result<()> {
    if reason.trim().is_empty() || reason.chars().count() > MAX_FEEDBACK_REASON_CHARS {
        return Err(RuntimeError::Invalid(format!(
            "reason must be nonblank and at most {MAX_FEEDBACK_REASON_CHARS} characters"
        )));
    }
    Ok(())
}

fn feedback_commit(
    spec: MemoryFeedbackSpec,
    by: Actor,
    run: aether_core::id::RunId,
) -> Result<(MemoryFeedback, Commit)> {
    debug_assert!(matches!(by, Actor::User { .. }));
    debug_assert!(!spec.reason.trim().is_empty());
    let mut payload = serde_json::to_value(&spec)
        .map_err(|error| RuntimeError::Internal(format!("encode feedback: {error}")))?;
    let record = MemoryFeedback {
        id: MemoryFeedbackId::new(),
        spec,
        actor: by.clone(),
        created_at: now(),
        retracted_at: None,
        retraction_reason: None,
    };
    payload["feedback"] = serde_json::json!(record.id);
    let event = EventDraft::new(
        by,
        EventKind::Custom {
            name: "memory.feedback".into(),
            payload,
        },
    )
    .task(record.spec.task)
    .run(run);
    let commit = Commit::new()
        .event(event)
        .mutate(Mutation::RecordMemoryFeedback(record.clone()));
    Ok((record, commit))
}
