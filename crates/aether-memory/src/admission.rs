use crate::{
    Admission, MemoryError, MemoryService, PROMOTION_SUPPORT, Result, require_operator, tiers,
};
use aether_core::{
    event::{Actor, EventDraft, EventKind},
    id::{AgentId, MemoryId, TaskId},
    memory::{MemoryCandidate, MemoryRecord},
    memory_admission::{MemoryAdmission, MemoryState, Supporter, Verification},
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
        let supporter = self.supporter(task)?;
        let (admission, mut commit) =
            self.evaluate_candidate(candidate, actor, superseded, false, supporter)?;
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
        supporter: Option<Supporter>,
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
        // The same text, still standing: a restatement, which supports it.
        let restated = old.as_ref().filter(|old| {
            old.is_current()
                && old.scope == candidate.scope
                && !superseded.contains(&old.id)
                && old.text.trim() == candidate.text.trim()
        });
        if let Some(reason) = reason {
            return self.refusal(commit, reason, actor, restated.zip(supporter));
        }
        debug_assert!(old.as_ref().is_none_or(MemoryRecord::is_current));
        debug_assert!(old.as_ref().is_none_or(|m| m.scope == candidate.scope));
        let inherited = restated.map(|old| self.admission(old)).transpose()?;
        let standing = Standing {
            proposed,
            inherited,
            supporter,
        };
        let (admission, admitted) = self.admission_commit(candidate, old, actor, standing)?;
        append(&mut commit, admitted);
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
        standing: Standing,
    ) -> Result<(Admission, Commit)> {
        if let Some(old) = &old {
            candidate.supersedes = Some(old.id);
        }
        let mut record = MemoryRecord::from_candidate(candidate, now());
        if standing.proposed {
            record.proposed_at = Some(record.created_at);
        }
        debug_assert_eq!(record.is_proposed(), standing.proposed);
        debug_assert!(record.forgotten_at.is_none() && record.superseded_by.is_none());
        let mut admission = self.admission_for(
            &record,
            &actor,
            standing.inherited.as_ref(),
            standing.supporter,
        );
        let promoted = tiers::promotable(&admission)
            .then(|| promotion(&mut admission, Verification::Observed, &actor));
        let mut commit = Commit::new()
            .mutate(Mutation::PutMemory(record.clone()))
            .mutate(Mutation::PutMemoryAdmission(admission.clone()))
            .event(EventDraft::new(
                actor.clone(),
                admission_event(&record, &admission),
            ));
        commit.events.extend(promoted);
        let Some(mut old) = old else {
            return Ok((Admission::Accepted { record }, commit));
        };
        // A candidate never displaces what the model already sees: it replaces
        // an active memory when it is promoted, not before.
        let deferred = admission.state == MemoryState::Candidate
            && self.admission(&old)?.state == MemoryState::Active;
        if standing.proposed || deferred {
            return Ok((Admission::Accepted { record }, commit));
        }
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
        let old = old.id;
        Ok((Admission::Superseded { record, old }, commit))
    }

    /// Not stored. A restatement from a task that has not supported the memory
    /// yet still counts for it, and says so instead of a rejection.
    fn refusal(
        &self,
        mut commit: Commit,
        reason: String,
        actor: Actor,
        restatement: Option<(&MemoryRecord, Supporter)>,
    ) -> Result<(Admission, Commit)> {
        if let Some((old, supporter)) = restatement
            && let Some((promoted, reinforced)) = self.reinforce(old, supporter, &actor)?
        {
            append(&mut commit, reinforced);
            let record = old.clone();
            return Ok((Admission::Reinforced { record, promoted }, commit));
        }
        debug_assert!(!reason.trim().is_empty());
        commit.push_event(EventDraft::new(
            actor,
            EventKind::MemoryRejected {
                reason: reason.clone(),
            },
        ));
        Ok((Admission::Rejected { reason }, commit))
    }

    /// Count `supporter` for a memory restated as it stands. `None` when its
    /// task already counted; otherwise whether that promoted it, and the commit.
    fn reinforce(
        &self,
        record: &MemoryRecord,
        supporter: Supporter,
        actor: &Actor,
    ) -> Result<Option<(bool, Commit)>> {
        debug_assert!(record.is_current());
        let mut admission = self.admission(record)?;
        if !admission.support(supporter) {
            return Ok(None);
        }
        let supported = EventKind::Custom {
            name: "memory.supported".into(),
            payload: serde_json::json!({
                "memory": record.id, "task": supporter.task,
                "tasks": admission.tasks(), "independent": admission.independent(),
            }),
        };
        let mut commit = Commit::new().event(EventDraft::new(actor.clone(), supported));
        let promoted = tiers::promotable(&admission);
        if promoted {
            append(&mut commit, self.promoted_replacement(record, actor)?);
            commit.push_event(promotion(&mut admission, Verification::Observed, actor));
        }
        debug_assert_eq!(admission.memory, record.id);
        commit.push_mutation(Mutation::PutMemoryAdmission(admission));
        Ok(Some((promoted, commit)))
    }

    /// The supersession a candidate deferred, applied now that it is promoted:
    /// nothing when its predecessor is gone, or may no longer be replaced.
    pub(crate) fn promoted_replacement(&self, record: &MemoryRecord, by: &Actor) -> Result<Commit> {
        let Some(id) = record.supersedes else {
            return Ok(Commit::new());
        };
        let Some(mut old) = self.store.memory(id)? else {
            return Ok(Commit::new());
        };
        if self.supersession_refused(&old, &record.scope, by).is_some() {
            return Ok(Commit::new());
        }
        debug_assert!(old.superseded_by.is_none() && old.id != record.id);
        old.superseded_by = Some(record.id);
        Ok(Commit::new()
            .mutate(Mutation::PutMemory(old))
            .event(EventDraft::new(
                by.clone(),
                EventKind::MemorySuperseded {
                    old: id,
                    new: record.id,
                    reason: None,
                },
            )))
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
            self.evaluate_candidate(candidate, actor, &BTreeSet::new(), proposed, None)?;
        let commit = if admission.record().is_some() {
            commit
        } else {
            Commit::new()
        };
        debug_assert_eq!(admission.record().is_some(), !commit.is_empty());
        Ok((admission, commit))
    }
}

/// How a new record is admitted, beyond its content.
struct Standing {
    proposed: bool,
    /// The admission of the record it restates, whose support it carries on.
    inherited: Option<MemoryAdmission>,
    supporter: Option<Supporter>,
}

fn append(commit: &mut Commit, more: Commit) {
    commit.events.extend(more.events);
    commit.mutations.extend(more.mutations);
}

/// Make a candidate active, at least as verified as `verification`, and say so.
pub(crate) fn promotion(
    admission: &mut MemoryAdmission,
    verification: Verification,
    by: &Actor,
) -> EventDraft {
    assert_eq!(
        admission.state,
        MemoryState::Candidate,
        "only a candidate is promoted"
    );
    admission.state = MemoryState::Active;
    admission.promoted_at = Some(now());
    admission.verification = admission.verification.max(verification);
    debug_assert!(admission.verification >= verification);
    EventDraft::new(
        by.clone(),
        EventKind::Custom {
            name: "memory.promoted".into(),
            payload: serde_json::json!({
                "memory": admission.memory, "tasks": admission.tasks(),
                "independent": admission.independent(),
            }),
        },
    )
}

fn admission_event(record: &MemoryRecord, admission: &MemoryAdmission) -> EventKind {
    debug_assert!(record.forgotten_at.is_none());
    debug_assert!(record.superseded_by.is_none());
    if !record.is_proposed() {
        let reason = (admission.state == MemoryState::Candidate)
            .then(|| format!("a candidate until {PROMOTION_SUPPORT} independent tasks state it"));
        return EventKind::MemoryAccepted {
            memory: record.id,
            reason,
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
