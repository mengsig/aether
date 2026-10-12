//! Curating memory: what a person keeps, drops, and imports.
//!
//! The store is the single writer here as everywhere else - the service
//! decides and commits, and the runtime only resolves what the caller named.

use std::path::PathBuf;

use aether_core::event::Actor;
use aether_core::id::MemoryId;
use aether_core::memory::{
    MemoryCandidate, MemoryProvenance, MemoryRecord, MemoryScope, OperatorMemoryScope,
    OperatorMemorySpec, SeedReport,
};
use aether_memory::Admission;

use crate::{Result, Runtime, RuntimeError};

impl Runtime {
    pub fn create_memory(&self, spec: OperatorMemorySpec, by: Actor) -> Result<MemoryRecord> {
        self.create_memory_keyed(spec, by, None)
    }

    pub fn create_memory_keyed(
        &self,
        spec: OperatorMemorySpec,
        by: Actor,
        submission: Option<aether_core::idempotency::Submission>,
    ) -> Result<MemoryRecord> {
        if let Some(hold) = self.hold_for(submission.as_ref())
            && let Some(receipt) = self.replay_receipt(&hold, submission.as_ref())?
        {
            return Ok(receipt);
        }
        if !matches!(by, Actor::User { .. }) {
            return Err(RuntimeError::Invalid(
                "only an operator can create operator memories".into(),
            ));
        }
        let importance = spec.importance.unwrap_or(0.5);
        let confidence = spec.confidence.unwrap_or(0.7);
        for (field, value) in [("importance", importance), ("confidence", confidence)] {
            if !(0.0..=1.0).contains(&value) {
                return Err(RuntimeError::Invalid(format!(
                    "{field} must be finite and between 0 and 1"
                )));
            }
        }
        if spec.text.trim().is_empty() {
            return Err(RuntimeError::Invalid("text is empty".into()));
        }
        if spec.tags.len() > 32
            || spec.tags.iter().any(|tag| {
                tag.trim().is_empty()
                    || tag.chars().count() > 64
                    || tag.chars().any(char::is_control)
            })
        {
            return Err(RuntimeError::Invalid(
                "tags must be at most 32 nonblank, control-free strings of at most 64 characters"
                    .into(),
            ));
        }
        let scope = match spec.scope {
            OperatorMemoryScope::Global => MemoryScope::Global,
            OperatorMemoryScope::Agent { id } => {
                self.store
                    .agent(id)?
                    .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?;
                MemoryScope::Agent { id }
            }
            OperatorMemoryScope::Repository { root } => {
                let (_, id) = self.memory_repository(root)?;
                MemoryScope::Repository { id }
            }
        };
        let candidate = MemoryCandidate {
            scope,
            kind: spec.kind,
            text: spec.text.trim().to_owned(),
            structured: None,
            importance,
            confidence,
            tags: spec.tags,
            provenance: MemoryProvenance {
                actor: Some(by.clone()),
                ..Default::default()
            },
            supersedes: spec.supersedes,
        };
        let hold = self.hold();
        if let Some(receipt) = self.replay_receipt(&hold, submission.as_ref())? {
            return Ok(receipt);
        }
        let (admission, commit) = self.memory.evaluate_operator(&hold, candidate, by)?;
        let record = match admission {
            Admission::Accepted { record } | Admission::Superseded { record, .. } => record,
            Admission::Rejected { reason } => {
                return Err(RuntimeError::Invalid(format!(
                    "memory not admitted: {reason}"
                )));
            }
        };
        self.commit_receipt(hold, commit, submission.as_ref(), &record)?;
        self.memory.index_committed(&record)?;
        Ok(record)
    }

    /// Curate a memory out. The record stays; it is never retrieved again.
    pub fn forget_memory(
        &self,
        id: MemoryId,
        reason: Option<String>,
        by: Actor,
    ) -> Result<MemoryRecord> {
        Ok(self.memory.forget(id, reason, by)?)
    }

    /// Put a forgotten memory back, so a mis-keyed curation has a way back.
    pub fn restore_memory(&self, id: MemoryId, by: Actor) -> Result<MemoryRecord> {
        Ok(self.memory.restore(id, by)?)
    }

    /// Keep a memory in front of the model, or stop.
    pub fn pin_memory(&self, id: MemoryId, pinned: bool, by: Actor) -> Result<MemoryRecord> {
        Ok(self.memory.pin(id, pinned, by)?)
    }

    /// Read a repository's committed notes into memory scoped to it. Answers
    /// with the scope id used, so the caller can query what it just imported.
    ///
    /// The root is one this daemon already works in, and the scope is derived
    /// from it. Reach to the socket is the whole authorization boundary
    /// (§16), so a caller free to name both could write notes into a
    /// directory of its own and admit them into any repository's memory as
    /// the person.
    pub fn sync_repository_knowledge(
        &self,
        root: Option<PathBuf>,
        by: Actor,
    ) -> Result<(String, SeedReport)> {
        let (root, id) = self.memory_repository(root)?;
        let report = self.memory.seed_repository(&root, &id, by)?;
        Ok((id, report))
    }

    fn memory_repository(&self, root: Option<PathBuf>) -> Result<(PathBuf, String)> {
        let root = match root {
            Some(named) => self.workspace_containing(&named)?,
            None => self.config.workspace_root.clone().ok_or_else(|| {
                RuntimeError::Invalid(
                    "no repository root: name one, or start the daemon in a workspace".into(),
                )
            })?,
        };
        if !root.is_dir() {
            return Err(RuntimeError::Invalid(format!(
                "{} is not a directory",
                root.display()
            )));
        }
        let id = crate::commands::repository_id(&root).ok_or_else(|| {
            RuntimeError::Invalid(format!("cannot name the repository at {}", root.display()))
        })?;
        Ok((root, id))
    }

    /// `root`, if it is a workspace this daemon works in - its own, or a live
    /// agent's - or inside one. Refused otherwise, naming what is allowed.
    fn workspace_containing(&self, root: &std::path::Path) -> Result<PathBuf> {
        let root = root
            .canonicalize()
            .map_err(|e| RuntimeError::Invalid(format!("{}: {e}", root.display())))?;
        let mut known: Vec<PathBuf> = self.config.workspace_root.clone().into_iter().collect();
        for agent in self.store.agents(&aether_store::AgentFilter::default())? {
            if !agent.status.is_terminal()
                && let Some(ws) = agent.scope.workspace_root
            {
                known.push(ws);
            }
        }
        let allowed = known
            .iter()
            .filter_map(|k| k.canonicalize().ok())
            .any(|k| root.starts_with(&k));
        if !allowed {
            return Err(RuntimeError::Invalid(format!(
                "{} is not a workspace this daemon works in; notes are read from the daemon's \
                 workspace or a live agent's",
                root.display()
            )));
        }
        Ok(root)
    }
}
