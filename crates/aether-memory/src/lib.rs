//! Durable memory: admission, supersession and retrieval.
//!
//! Persist broadly, inject sparingly. Most completed tasks produce no memory
//! at all. A candidate is admitted only when it is stable, non-trivial, and
//! not already represented; retrieval is structural first (scope, tags,
//! recency) and lexical second, and never puts more than a handful of facts
//! in front of a model.

pub mod index;
pub mod repo;

pub use index::{LocalEmbeddingIndex, MemoryIndex};

use std::collections::BTreeSet;
use std::sync::Arc;

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::{AgentId, MemoryId, TaskId};
use aether_core::memory::{MemoryCandidate, MemoryKind, MemoryRecord, MemoryScope};
use aether_core::time::now;
use aether_store::{Commit, Hold, MemoryQuery, Mutation, Store, StoreError, WriteGate};

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("{0}")]
    Invalid(String),
    #[error("memory {0} not found")]
    NotFound(MemoryId),
    #[error("repository knowledge: {0}")]
    RepoKnowledge(#[from] repo::RepoKnowledgeError),
    #[error("memory index `{index}`: {detail}")]
    Index { index: String, detail: String },
}

pub type Result<T> = std::result::Result<T, MemoryError>;

/// Thresholds for admission. Conservative by default.
#[derive(Debug, Clone, PartialEq)]
pub struct AdmissionPolicy {
    pub min_importance: f32,
    pub min_confidence: f32,
    pub min_chars: usize,
    pub max_chars: usize,
    /// Token-set similarity a duplicate must also reach. Similarity alone never
    /// makes a duplicate: only the same text, trimmed, can be one.
    pub duplicate_threshold: f32,
    /// Phrases that mark a fact as transient rather than durable.
    pub transient_markers: Vec<&'static str>,
}

impl Default for AdmissionPolicy {
    fn default() -> Self {
        Self {
            min_importance: 0.3,
            min_confidence: 0.4,
            min_chars: 12,
            max_chars: 1_200,
            duplicate_threshold: 0.8,
            transient_markers: vec![
                "right now",
                "for now",
                "currently",
                "temporarily",
                "at the moment",
                "while debugging",
                "todo:",
                "in progress",
            ],
        }
    }
}

/// Outcome of considering a candidate.
#[derive(Debug, Clone, PartialEq)]
pub enum Admission {
    Accepted {
        record: MemoryRecord,
    },
    /// Accepted and an older record was marked superseded.
    Superseded {
        record: MemoryRecord,
        old: MemoryId,
    },
    Rejected {
        reason: String,
    },
}

impl Admission {
    pub fn record(&self) -> Option<&MemoryRecord> {
        match self {
            Self::Accepted { record } | Self::Superseded { record, .. } => Some(record),
            Self::Rejected { .. } => None,
        }
    }
}

/// A retrieved memory with its relevance score.
#[derive(Debug, Clone, PartialEq)]
pub struct Retrieved {
    pub record: MemoryRecord,
    pub score: f32,
}

/// What an agent is doing, for scoring retrieval.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RetrievalRequest {
    pub scope: Option<MemoryScope>,
    pub role: Option<String>,
    pub agent: Option<AgentId>,
    /// `agent`'s ancestors, nearest first. Their `Subtree` memories are in
    /// scope, up to [`MAX_LINEAGE`] generations up.
    pub ancestors: Vec<AgentId>,
    /// Free text: the objective, recent messages, file names.
    pub query: Option<String>,
    pub tags: Vec<String>,
    pub kinds: Vec<MemoryKind>,
    pub limit: usize,
}

/// The memory service. Decides; the runtime commits (single writer), except
/// for [`MemoryService::admit`], a convenience that commits directly.
#[derive(Clone)]
pub struct MemoryService {
    store: Arc<dyn Store>,
    /// The write path this service shares with everything else committing to
    /// `store`. Curation reads a record and writes the whole row back, so the
    /// read and the commit have to be one step.
    gate: WriteGate,
    policy: AdmissionPolicy,
    index: Option<Arc<dyn MemoryIndex>>,
}

impl std::fmt::Debug for MemoryService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryService")
            .field("policy", &self.policy)
            .field("index", &self.index.as_ref().map(|i| i.name()))
            .finish()
    }
}

impl MemoryService {
    pub fn new(store: Arc<dyn Store>, gate: WriteGate) -> Self {
        Self {
            store,
            gate,
            policy: AdmissionPolicy::default(),
            index: None,
        }
    }

    pub fn with_policy(mut self, policy: AdmissionPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Add a semantic pass to retrieval. Without one, retrieval is structural
    /// and lexical, which is the default everywhere.
    pub fn with_index(mut self, index: Arc<dyn MemoryIndex>) -> Self {
        self.index = Some(index);
        self
    }

    /// `scope` with its repository resolved through the registered aliases,
    /// so whoever still names an old repository reads and writes the scope its
    /// memories were moved to. Every other scope comes back as it went in.
    pub fn canonical_scope(&self, scope: MemoryScope) -> Result<MemoryScope> {
        let resolved = match scope {
            MemoryScope::Repository { id } => MemoryScope::Repository {
                id: self.store.resolve_repository(&id)?,
            },
            MemoryScope::RepositoryRole { id, role } => MemoryScope::RepositoryRole {
                id: self.store.resolve_repository(&id)?,
                role,
            },
            other => other,
        };
        Ok(resolved)
    }

    /// `read` against `scope` resolved through the aliases, outside any hold.
    /// An alias committing mid-read changes the resolution, so the read is
    /// repeated once against the new target; aliases are permanent and never
    /// chain, so a name's resolution changes at most once.
    pub fn read_resolved<T>(
        &self,
        scope: Option<MemoryScope>,
        read: impl Fn(Option<MemoryScope>) -> Result<T>,
    ) -> Result<T> {
        let resolve =
            |scope: Option<MemoryScope>| scope.map(|s| self.canonical_scope(s)).transpose();
        let first = resolve(scope.clone())?;
        let found = read(first.clone())?;
        let again = resolve(scope)?;
        if again == first {
            return Ok(found);
        }
        debug_assert_eq!(
            resolve(again.clone())?,
            again,
            "an alias resolved to an alias"
        );
        read(again)
    }

    /// Evaluate and commit, taking the hold itself. For a caller that has no
    /// commit of its own to fold the decision into.
    pub fn admit(
        &self,
        candidate: MemoryCandidate,
        actor: Actor,
        agent: Option<AgentId>,
        task: Option<TaskId>,
    ) -> Result<Admission> {
        let admission = {
            let hold = self.gate.hold();
            let (admission, commit) = self.evaluate(&hold, candidate, actor, agent, task)?;
            self.store.commit(commit)?;
            admission
        };
        if let Some(record) = admission.record() {
            self.index_committed(record)?;
        }
        Ok(admission)
    }

    /// Index only after committing: the index must never expose an uncommitted record.
    pub fn index_committed(&self, record: &MemoryRecord) -> Result<()> {
        if record.is_current()
            && let Some(index) = &self.index
        {
            index.index(record)?;
        }
        Ok(())
    }

    /// Who may replace an existing memory with a new one. A pinned record and
    /// a note the repository states in its own files are decisions, not
    /// guesses: an agent that could supersede them could talk itself out of
    /// the constraint that was meant to stop it. A person still can.
    fn supersession_refused(
        &self,
        old: &MemoryRecord,
        scope: &MemoryScope,
        actor: &Actor,
    ) -> Option<String> {
        if !old.is_current() || &old.scope != scope {
            return Some(format!(
                "memory {} must be current and in the same scope to replace it",
                old.id
            ));
        }
        if matches!(actor, Actor::User { .. }) {
            return None;
        }
        if old
            .provenance
            .actor
            .as_ref()
            .is_some_and(|actor| matches!(actor.original(), Actor::User { .. }))
        {
            return Some(format!(
                "memory {} was authored by an operator; only a person can replace it",
                old.id
            ));
        }
        if old.pinned {
            return Some(format!(
                "memory {} is pinned; only a person can replace it",
                old.id
            ));
        }
        if old.provenance.source.is_some() {
            return Some(format!(
                "memory {} states what {} says; change the file, or ask a person",
                old.id,
                old.provenance
                    .source
                    .as_ref()
                    .map(|s| s.file.as_str())
                    .unwrap_or("the repository")
            ));
        }
        None
    }

    /// The candidate as it is safe to record: text past the admission cap is
    /// truncated, because a candidate that long is rejected anyway and the
    /// event exists to say what was offered, not to store it.
    fn bounded(&self, c: &MemoryCandidate) -> MemoryCandidate {
        if c.text.chars().count() <= self.policy.max_chars {
            return c.clone();
        }
        let mut bounded = c.clone();
        bounded.text = c
            .text
            .chars()
            .take(self.policy.max_chars)
            .chain(" [truncated]".chars())
            .collect();
        bounded
    }

    fn reject_reason(&self, c: &MemoryCandidate) -> Option<String> {
        let text = c.text.trim();
        if text.chars().count() < self.policy.min_chars {
            return Some("too short to be a durable fact".into());
        }
        if text.chars().count() > self.policy.max_chars {
            return Some("too long; store as an artifact and remember a summary".into());
        }
        if c.importance < self.policy.min_importance {
            return Some(format!("importance {:.2} below threshold", c.importance));
        }
        if c.confidence < self.policy.min_confidence {
            return Some(format!("confidence {:.2} below threshold", c.confidence));
        }
        let lower = text.to_lowercase();
        if let Some(marker) = self
            .policy
            .transient_markers
            .iter()
            .find(|m| lower.contains(*m))
        {
            return Some(format!("looks transient (contains \"{marker}\")"));
        }
        None
    }

    /// A changed word, number, path, case or punctuation mark may be a changed
    /// requirement, so only the same text, trimmed, can duplicate a memory.
    fn find_duplicate(
        &self,
        scope: &MemoryScope,
        text: &str,
        exclude: Option<MemoryId>,
    ) -> Result<Option<MemoryRecord>> {
        let existing = self.store.memories(&MemoryQuery {
            scope: Some(scope.clone()),
            exact_scope: true,
            text: Some(text.into()),
            limit: 10,
            ..Default::default()
        })?;
        let text = text.trim();
        let candidate_tokens = tokens(text);
        let best = existing
            .into_iter()
            .filter(|m| Some(m.id) != exclude)
            .filter(|m| m.text.trim() == text)
            .map(|m| {
                let sim = jaccard(&candidate_tokens, &tokens(&m.text));
                (sim, m)
            })
            .filter(|(sim, _)| *sim >= self.policy.duplicate_threshold)
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, m)| m);
        debug_assert!(best.as_ref().is_none_or(|m| &m.scope == scope));
        debug_assert!(best.as_ref().is_none_or(MemoryRecord::is_current));
        Ok(best)
    }

    /// Curate a memory out. The record stays - history is never deleted - but
    /// it is never retrieved again. Returns the updated record.
    pub fn forget(&self, id: MemoryId, reason: Option<String>, by: Actor) -> Result<MemoryRecord> {
        let (record, changed) = self.curate(id, by, |record| {
            let already = record.forgotten_at.is_some();
            record.forgotten_at = Some(now());
            (!already, EventKind::MemoryForgotten { memory: id, reason })
        })?;
        // After the commit: an index that dropped a record the commit then
        // failed to write would stop retrieving a memory that is still live.
        if changed && let Some(index) = &self.index {
            index.forget(id)?;
        }
        Ok(record)
    }

    /// Restore a memory a person curated out. The inverse of [`Self::forget`],
    /// because a mis-keyed curation otherwise has no way back.
    pub fn restore(&self, id: MemoryId, by: Actor) -> Result<MemoryRecord> {
        let (record, changed) = self.curate(id, by, |record| {
            let was = record.forgotten_at.take().is_some();
            // A person putting it back outranks the sweep that took it out.
            record.retracted = None;
            (was, EventKind::MemoryRestored { memory: id })
        })?;
        if changed
            && record.is_current()
            && let Some(index) = &self.index
        {
            index.index(&record)?;
        }
        Ok(record)
    }

    /// Keep a memory in front of the model, or stop. Returns the record.
    pub fn pin(&self, id: MemoryId, pinned: bool, by: Actor) -> Result<MemoryRecord> {
        Ok(self
            .curate(id, by, |record| {
                let changed = record.pinned != pinned;
                record.pinned = pinned;
                (changed, EventKind::MemoryPinned { memory: id, pinned })
            })?
            .0)
    }

    /// Read a record, change one field, and commit the whole row back - all
    /// under one hold. `change` returns whether anything moved and the event
    /// that says so; nothing is written when nothing moved.
    ///
    /// The hold is the point: `Mutation::PutMemory` replaces every column, so
    /// a pin read outside it and committed after a supersession would put the
    /// superseded record back in front of the model, pinned.
    fn curate(
        &self,
        id: MemoryId,
        by: Actor,
        change: impl FnOnce(&mut MemoryRecord) -> (bool, EventKind),
    ) -> Result<(MemoryRecord, bool)> {
        let _hold = self.gate.hold();
        let mut record = self.store.memory(id)?.ok_or(MemoryError::NotFound(id))?;
        let (changed, event) = change(&mut record);
        if !changed {
            return Ok((record, false));
        }
        self.store.commit(
            Commit::new()
                .event(EventDraft::new(by, event))
                .mutate(Mutation::PutMemory(record.clone())),
        )?;
        Ok((record, true))
    }

    /// Record that memories were shown to a model. Returns a commit for the
    /// runtime to apply alongside the turn, under the same hold: each record
    /// is read here and written whole there.
    pub fn touch(&self, _hold: &Hold<'_>, ids: &[MemoryId]) -> Result<Commit> {
        let at = now();
        let mut commit = Commit::new();
        for id in ids {
            if let Some(mut record) = self.store.memory(*id)? {
                record.last_retrieved_at = Some(at);
                record.retrieval_count += 1;
                commit.push_mutation(Mutation::PutMemory(record));
            }
        }
        Ok(commit)
    }

    /// Render retrieved memories for a prompt layer.
    pub fn render(items: &[Retrieved]) -> String {
        let mut out = String::new();
        for item in items {
            let r = &item.record;
            out.push_str("- [");
            out.push_str(&r.id.to_string());
            out.push_str("] ");
            out.push_str(&r.text);
            if !r.tags.is_empty() {
                out.push_str(" (");
                out.push_str(&r.tags.join(", "));
                out.push(')');
            }
            out.push('\n');
        }
        out
    }
}

/// How many generations up an agent sees its ancestors' `Subtree` memories.
/// Each is one more scope asked on every turn; a deep orchestration chain
/// (orchestrator, steward, request worker, reviewer) is three.
pub const MAX_LINEAGE: usize = 8;

/// Words worth matching on. Function words appear in every memory, so
/// counting them makes an unrelated sentence look like a hit.
fn tokens(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|t| index::carries_meaning(t))
        .collect()
}

fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let inter = a.intersection(b).count() as f32;
    let union = a.union(b).count() as f32;
    inter / union
}

mod admission;
mod proposals;
mod retrieval;
mod seeding;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod repository_knowledge_tests;

#[cfg(test)]
mod curation_tests;

#[cfg(test)]
mod embedding_tests;

#[cfg(test)]
mod feedback_ranking_tests;

#[cfg(test)]
mod admission_tests;

fn require_operator(actor: &Actor) -> Result<()> {
    if !matches!(actor, Actor::User { .. }) {
        return Err(MemoryError::Invalid(
            "only an operator can review memory proposals".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod proposal_tests;
