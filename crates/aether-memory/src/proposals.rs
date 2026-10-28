use crate::{MemoryError, MemoryService, Result, require_operator};
use aether_core::{
    event::{Actor, EventDraft, EventKind},
    id::MemoryId,
    memory::MemoryRecord,
    time::now,
};
use aether_store::{Commit, Hold, Mutation};

impl MemoryService {
    /// Accept and replace under the same gate; retries never write another event.
    pub fn accept(&self, id: MemoryId, reason: Option<String>, by: Actor) -> Result<MemoryRecord> {
        require_operator(&by)?;
        validate_reason(reason.as_deref())?;
        let record = {
            let hold = self.gate.hold();
            let mut record = self.proposal(id)?;
            if record.forgotten_at.is_some() || record.superseded_by.is_some() {
                return Err(MemoryError::Invalid(format!(
                    "memory {id} is forgotten or superseded"
                )));
            }
            if record.accepted_at.is_none() {
                self.commit_acceptance(&hold, &mut record, reason, by)?;
            }
            debug_assert!(record.is_current());
            debug_assert!(record.accepted_at.is_some());
            record
        };
        self.index_committed(&record)?;
        Ok(record)
    }

    fn commit_acceptance(
        &self,
        hold: &Hold<'_>,
        record: &mut MemoryRecord,
        reason: Option<String>,
        by: Actor,
    ) -> Result<()> {
        debug_assert!(record.is_proposed());
        debug_assert!(record.forgotten_at.is_none());
        let scope = self.canonical_scope(record.scope.clone())?;
        if let Some(existing) = self.find_duplicate(&scope, &record.text, record.supersedes)? {
            return Err(MemoryError::Invalid(format!(
                "proposal {} duplicates existing memory {}",
                record.id, existing.id
            )));
        }
        let mut commit = self.deferred_supersession(record, &by, reason.clone())?;
        record.accepted_at = Some(now());
        debug_assert!(record.is_current());
        commit.push_mutation(Mutation::PutMemory(record.clone()));
        commit.push_event(EventDraft::new(
            by,
            EventKind::MemoryAccepted {
                memory: record.id,
                reason,
            },
        ));
        self.commit(hold, commit)?;
        Ok(())
    }

    fn deferred_supersession(
        &self,
        record: &MemoryRecord,
        by: &Actor,
        reason: Option<String>,
    ) -> Result<Commit> {
        debug_assert!(record.is_proposed());
        debug_assert!(record.forgotten_at.is_none());
        let Some(id) = record.supersedes else {
            return Ok(Commit::new());
        };
        let mut old = self.store.memory(id)?.ok_or(MemoryError::NotFound(id))?;
        let scope = self.canonical_scope(record.scope.clone())?;
        if let Some(reason) = self.supersession_refused(&old, &scope, by) {
            return Err(MemoryError::Invalid(reason));
        }
        debug_assert_ne!(old.id, record.id);
        debug_assert!(old.is_current());
        old.superseded_by = Some(record.id);
        Ok(Commit::new()
            .mutate(Mutation::PutMemory(old))
            .event(EventDraft::new(
                by.clone(),
                EventKind::MemorySuperseded {
                    old: id,
                    new: record.id,
                    reason,
                },
            )))
    }

    /// Rejecting keeps the pending marker, so restore reopens review only.
    pub fn reject(&self, id: MemoryId, reason: String, by: Actor) -> Result<MemoryRecord> {
        require_operator(&by)?;
        validate_reason(Some(&reason))?;
        let hold = self.gate.hold();
        let mut record = self.proposal(id)?;
        if !record.is_proposed() {
            return Err(MemoryError::Invalid(format!(
                "memory {id} was already accepted"
            )));
        }
        if record.forgotten_at.is_some() {
            return Ok(record);
        }
        debug_assert!(record.accepted_at.is_none());
        debug_assert!(record.superseded_by.is_none());
        record.forgotten_at = Some(now());
        self.commit(
            &hold,
            Commit::new()
                .mutate(Mutation::PutMemory(record.clone()))
                .event(EventDraft::new(
                    by.clone(),
                    EventKind::Custom {
                        name: "memory.proposal_rejected".into(),
                        payload: serde_json::json!({"memory":id,"reason":reason}),
                    },
                ))
                .event(EventDraft::new(
                    by,
                    EventKind::MemoryForgotten {
                        memory: id,
                        reason: Some(reason),
                    },
                )),
        )?;
        Ok(record)
    }

    fn proposal(&self, id: MemoryId) -> Result<MemoryRecord> {
        let record = self.store.memory(id)?.ok_or(MemoryError::NotFound(id))?;
        if record.proposed_at.is_none() {
            return Err(MemoryError::Invalid(format!(
                "memory {id} was never proposed"
            )));
        }
        debug_assert_eq!(record.id, id);
        debug_assert!(
            record
                .provenance
                .actor
                .as_ref()
                .is_some_and(|by| matches!(by.original(), Actor::User { .. }))
        );
        Ok(record)
    }
}

fn validate_reason(reason: Option<&str>) -> Result<()> {
    if reason.is_some_and(|reason| reason.trim().is_empty()) {
        return Err(MemoryError::Invalid("reason must not be blank".into()));
    }
    Ok(())
}
