use crate::{Admission, MemoryError, MemoryService, Result, require_operator};
use aether_core::{
    event::{Actor, EventDraft, EventKind},
    id::{AgentId, MemoryId, TaskId},
    memory::{MemoryCandidate, MemoryRecord},
    time::now,
};
use aether_store::{Commit, Hold, Mutation};
use std::collections::BTreeSet;

impl MemoryService {
    /// Evaluate without writing; the caller retains the hold through its commit.
    /// The predecessor is read here and replaced by that commit.
    pub fn evaluate(
        &self,
        hold: &Hold<'_>,
        candidate: MemoryCandidate,
        actor: Actor,
        agent: Option<AgentId>,
        task: Option<TaskId>,
    ) -> Result<(Admission, Commit)> {
        self.evaluate_batch(hold, candidate, actor, agent, task, &BTreeSet::new())
    }

    /// Evaluate against durable records and replacements already selected in this commit.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_batch(
        &self,
        _hold: &Hold<'_>,
        candidate: MemoryCandidate,
        actor: Actor,
        agent: Option<AgentId>,
        task: Option<TaskId>,
        superseded: &BTreeSet<MemoryId>,
    ) -> Result<(Admission, Commit)> {
        let (admission, mut commit) =
            self.evaluate_candidate(candidate, actor, superseded, false)?;
        for event in &mut commit.events {
            event.agent_id = agent;
            event.task_id = task;
        }
        debug_assert!(commit.events.iter().all(|e| e.agent_id == agent));
        debug_assert!(commit.events.iter().all(|e| e.task_id == task));
        Ok((admission, commit))
    }

    fn evaluate_candidate(
        &self,
        candidate: MemoryCandidate,
        actor: Actor,
        superseded: &BTreeSet<MemoryId>,
        proposed: bool,
    ) -> Result<(Admission, Commit)> {
        let candidate = MemoryCandidate {
            scope: self.canonical_scope(candidate.scope)?,
            ..candidate
        };
        let mut commit = Commit::new().event(EventDraft::new(
            actor.clone(),
            EventKind::MemoryCandidate {
                candidate: self.bounded(&candidate),
            },
        ));
        let mut reason = self.reject_reason(&candidate);
        let old = if reason.is_some() {
            None
        } else {
            match candidate.supersedes {
                Some(id) => Some(self.store.memory(id)?.ok_or(MemoryError::NotFound(id))?),
                None => self.find_duplicate(&candidate.scope, &candidate.text, None)?,
            }
        };
        if let Some(old) = &old {
            reason = self.replacement_reason(old, &candidate, &actor, superseded);
        }
        if let Some(reason) = reason {
            commit.push_event(EventDraft::new(
                actor,
                EventKind::MemoryRejected {
                    reason: reason.clone(),
                },
            ));
            return Ok((Admission::Rejected { reason }, commit));
        }
        debug_assert!(old.as_ref().is_none_or(MemoryRecord::is_current));
        debug_assert!(old.as_ref().is_none_or(|m| m.scope == candidate.scope));
        let (admission, admitted) = self.admission_commit(candidate, old, actor, proposed);
        commit.events.extend(admitted.events);
        commit.mutations.extend(admitted.mutations);
        Ok((admission, commit))
    }

    fn replacement_reason(
        &self,
        old: &MemoryRecord,
        candidate: &MemoryCandidate,
        actor: &Actor,
        superseded: &BTreeSet<MemoryId>,
    ) -> Option<String> {
        if superseded.contains(&old.id) {
            return Some(format!(
                "memory {} is already superseded in this batch",
                old.id
            ));
        }
        if let Some(reason) = self.supersession_refused(old, &candidate.scope, actor) {
            return Some(reason);
        }
        debug_assert!(old.is_current());
        debug_assert_eq!(old.scope, candidate.scope);
        if candidate.supersedes.is_none()
            && candidate.importance <= old.importance
            && candidate.confidence <= old.confidence
        {
            return Some(format!("redundant with existing memory {}", old.id));
        }
        None
    }

    fn admission_commit(
        &self,
        mut candidate: MemoryCandidate,
        old: Option<MemoryRecord>,
        actor: Actor,
        proposed: bool,
    ) -> (Admission, Commit) {
        if let Some(old) = &old {
            candidate.supersedes = Some(old.id);
        }
        let mut record = MemoryRecord::from_candidate(candidate, now());
        if proposed {
            record.proposed_at = Some(record.created_at);
        }
        debug_assert_eq!(record.is_proposed(), proposed);
        debug_assert!(record.forgotten_at.is_none() && record.superseded_by.is_none());
        let event = admission_event(&record);
        let mut commit = Commit::new()
            .mutate(Mutation::PutMemory(record.clone()))
            .event(EventDraft::new(actor.clone(), event));
        if !proposed && let Some(mut old) = old {
            old.superseded_by = Some(record.id);
            commit.push_mutation(Mutation::PutMemory(old.clone()));
            commit.push_event(EventDraft::new(
                actor,
                EventKind::MemorySuperseded {
                    old: old.id,
                    new: record.id,
                    reason: None,
                },
            ));
            return (
                Admission::Superseded {
                    record,
                    old: old.id,
                },
                commit,
            );
        }
        (Admission::Accepted { record }, commit)
    }

    /// Evaluate operator content under the caller's hold. Rejections have no commit.
    pub fn evaluate_operator(
        &self,
        _hold: &Hold<'_>,
        candidate: MemoryCandidate,
        actor: Actor,
        proposed: bool,
    ) -> Result<(Admission, Commit)> {
        require_operator(&actor)?;
        debug_assert_eq!(candidate.provenance.actor.as_ref(), Some(&actor));
        let (admission, commit) =
            self.evaluate_candidate(candidate, actor, &BTreeSet::new(), proposed)?;
        let commit = if admission.record().is_some() {
            commit
        } else {
            Commit::new()
        };
        debug_assert_eq!(admission.record().is_some(), !commit.is_empty());
        Ok((admission, commit))
    }
}

fn admission_event(record: &MemoryRecord) -> EventKind {
    debug_assert!(record.forgotten_at.is_none());
    debug_assert!(record.superseded_by.is_none());
    if !record.is_proposed() {
        return EventKind::MemoryAccepted {
            memory: record.id,
            reason: None,
        };
    }
    let mut payload = serde_json::json!({
        "memory": record.id, "scope": record.scope, "tags": record.tags,
    });
    if let Some(id) = record.supersedes {
        payload["supersedes"] = serde_json::json!(id);
    }
    EventKind::Custom {
        name: "memory.proposed".into(),
        payload,
    }
}
