//! Standing at admission: which memories start as candidates, what a record's
//! admission is when the store has none, and the support that promotes one.

use crate::{MemoryService, Result};
use aether_core::{
    event::Actor,
    id::TaskId,
    memory::MemoryRecord,
    memory_admission::{MemoryAdmission, MemoryState, Supporter},
};

/// Independent tasks (distinct runs) that must state a candidate before it
/// is put in front of a model.
pub const PROMOTION_SUPPORT: u32 = 2;

/// Whether an agent's memory is injected at once or waits for support.
/// Immediate until the `memory_v2` feature turns candidates on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AdmissionMode {
    #[default]
    Immediate,
    Candidate,
}

impl MemoryService {
    /// The admission the store keeps for `id`. A record from before admission
    /// was tracked, or written without one, stands as it always did: active.
    pub fn admission(&self, record: &MemoryRecord) -> Result<MemoryAdmission> {
        let stored = self.store.memory_admission(record.id)?;
        debug_assert!(stored.as_ref().is_none_or(|a| a.memory == record.id));
        Ok(stored.unwrap_or_else(|| MemoryAdmission::derived(record, MemoryState::Active)))
    }

    /// `task` as a source of support. A task the store does not know cannot
    /// show it is independent of the others, so it supports nothing.
    pub(crate) fn supporter(&self, task: Option<TaskId>) -> Result<Option<Supporter>> {
        let Some(id) = task else {
            return Ok(None);
        };
        let supporter = self.store.task(id)?.map(|task| Supporter {
            task: task.id,
            run: task.run,
        });
        debug_assert!(supporter.is_none_or(|s| s.task == id));
        Ok(supporter)
    }

    /// The admission of a new `record` written by `actor`. A restatement of
    /// `inherited` keeps its support and never stands lower than it did.
    pub(crate) fn admission_for(
        &self,
        record: &MemoryRecord,
        actor: &Actor,
        inherited: Option<&MemoryAdmission>,
        supporter: Option<Supporter>,
    ) -> MemoryAdmission {
        let state = if self.starts_as_candidate(record, actor) {
            MemoryState::Candidate
        } else {
            MemoryState::Active
        };
        let mut admission = MemoryAdmission::derived(record, state);
        if let Some(old) = inherited {
            debug_assert_ne!(old.memory, record.id, "a record never inherits itself");
            admission.support = old.support.clone();
            admission.verification = admission.verification.max(old.verification);
            if old.state == MemoryState::Active {
                admission.state = MemoryState::Active;
                admission.promoted_at = old.promoted_at;
            }
        }
        if let Some(supporter) = supporter {
            admission.support(supporter);
        }
        debug_assert_eq!(admission.memory, record.id);
        admission
    }

    /// Only a model's own claim waits for support: an operator's word and a
    /// note read from a committed file are trusted as they are.
    fn starts_as_candidate(&self, record: &MemoryRecord, actor: &Actor) -> bool {
        self.policy.mode == AdmissionMode::Candidate
            && !matches!(actor, Actor::User { .. } | Actor::System)
            && record.provenance.source.is_none()
    }
}

/// A candidate enough independent tasks have stated.
pub(crate) fn promotable(admission: &MemoryAdmission) -> bool {
    admission.state == MemoryState::Candidate && admission.independent() >= PROMOTION_SUPPORT
}
