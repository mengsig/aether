use crate::{Admission, MemoryService, Result, repo};
use aether_core::{
    event::{Actor, EventKind},
    id::MemoryId,
    memory::{MemoryCandidate, MemoryRecord, MemoryScope, Retraction, SeedReport},
    time::now,
};
use aether_store::{Hold, MemoryQuery};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// How many of a scope's memories a retraction sweep looks at. A repository
/// with more seeded notes than this has already hit the scan caps in `repo`.
const MAX_SEEDED_PER_SCOPE: usize = 1_000;

impl MemoryService {
    /// Read a repository's committed notes and admit what is not already
    /// there. Idempotent: an unchanged note is skipped before it becomes a
    /// candidate, so re-seeding an unchanged tree writes nothing at all.
    ///
    /// A note that has left the repository's committed tree is retracted,
    /// because the label (`file@sha`) promises the fact can be checked
    /// against the line that states it, and a reverted commit leaves no such
    /// line. See [`Self::retract_departed_notes`] for what "left" means when
    /// the scope is shared by every clone.
    ///
    /// Notes land in `repository`'s scope, or its role scopes for notes filed
    /// under a role directory ([`repo::scan`]).
    pub fn seed_repository(
        &self,
        root: &Path,
        repository: &str,
        actor: Actor,
    ) -> Result<SeedReport> {
        // Resolved first: the presence check and the sweep ask by scope, and a
        // checkout whose remote is an alias must find the notes moved away.
        let repository = self.store.resolve_repository(repository)?;
        let repository = repository.as_str();
        let candidates = repo::scan(root, repository)?;
        let mut report = SeedReport {
            found: candidates.len(),
            ..Default::default()
        };
        for candidate in &candidates {
            // Presence, admission and the commit are one step: two agents
            // created in one workspace would otherwise both see an empty
            // scope and admit every note twice.
            let hold = self.gate.hold();
            // Again under the gate the alias move takes: one may have moved
            // this note since the name was resolved above.
            let candidate = MemoryCandidate {
                scope: self.canonical_scope(candidate.scope.clone())?,
                ..candidate.clone()
            };
            if self.already_seeded(&hold, &candidate)? {
                report.unchanged += 1;
                continue;
            }
            let (admission, commit) = self.evaluate(&hold, candidate, actor.clone(), None, None)?;
            self.commit(&hold, commit)?;
            drop(hold);
            match &admission {
                Admission::Accepted { .. } => report.admitted += 1,
                Admission::Superseded { .. } => report.superseded += 1,
                Admission::Rejected { .. } => report.rejected += 1,
            }
            if let (Some(index), Some(record)) = (&self.index, admission.record()) {
                index.index(record)?;
            }
        }
        report.retracted = self.retract_departed_notes(root, repository, &actor)?;
        Ok(report)
    }

    /// Whether this exact note is already stored in its scope, in any state a
    /// person put it in. A keyed lookup, not a page of the scope: superseded
    /// and forgotten records count as present, so a person's decision to
    /// curate a note out outranks the file however many memories the scope
    /// has since accumulated. A superseded repository note also stays blocked
    /// from re-seeding: the replacement is the current source of truth.
    ///
    /// A record the sweep retracted does not count: it was curated out for
    /// having left the tree, and the note is back.
    fn already_seeded(&self, _hold: &Hold<'_>, candidate: &MemoryCandidate) -> Result<bool> {
        Ok(self
            .store
            .memories(&MemoryQuery {
                scope: Some(candidate.scope.clone()),
                exact_scope: true,
                exact_text: Some(candidate.text.clone()),
                include_superseded: true,
                include_forgotten: true,
                // Every state this one text has been in, not a page of them:
                // a person's forget must not fall off the end behind a run of
                // sweep retractions. Keyed and exact, so it returns very few.
                limit: MAX_SEEDED_PER_SCOPE,
                ..Default::default()
            })?
            .iter()
            .any(|m| m.retracted.is_none()))
    }

    /// Retract every live memory in `repository`'s scope, or any role scope
    /// within it, that came from a committed note the repository no longer
    /// states. The role scopes are asked by key, not by the directories in
    /// front of it, so deleting a whole role directory retracts its notes.
    /// History is kept, with the commit checked: the note was checkable when
    /// it was read and is not any more.
    ///
    /// Bounded to one page of the scope (`MAX_SEEDED_PER_SCOPE` records): past
    /// that many seeded records in one repository scope, a departed note past
    /// the page is not retracted this sync. A known ceiling, not paginated
    /// (ARCHITECTURE.md §10).
    ///
    /// Keyed to `root`'s committed tree, never to the files in front of it. A
    /// repository scope is shared by every clone, worktree and branch, so an
    /// agent started on an older or divergent checkout must not forget notes
    /// for all of them. A record is swept only when this checkout's history
    /// contains the commit it was read at - so the tree has seen it - and
    /// HEAD's own content of the file no longer states it. A note whose
    /// commit is unknown is not checkable and is never swept, and neither is
    /// a pinned one, which is a person's decision.
    fn retract_departed_notes(
        &self,
        root: &Path,
        repository: &str,
        actor: &Actor,
    ) -> Result<usize> {
        let Some(head) = repo::head_commit(root) else {
            return Ok(0);
        };
        let scope = MemoryScope::Repository {
            id: repository.to_owned(),
        };
        let seeded: Vec<MemoryRecord> = self
            .read_resolved(Some(scope), |scope| {
                self.store
                    .memories(&MemoryQuery {
                        scope,
                        exact_scope: true,
                        nested_scopes: true,
                        limit: MAX_SEEDED_PER_SCOPE,
                        ..Default::default()
                    })
                    .map_err(Into::into)
            })?
            .into_iter()
            .filter(|m| m.provenance.source.is_some() && !m.pinned)
            .collect();
        let mut stated: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut retracted = 0;
        for record in seeded {
            let Some(source) = record.provenance.source.clone() else {
                continue;
            };
            let Some(read_at) = source.commit.as_deref() else {
                continue;
            };
            if !repo::contains_commit(root, read_at) {
                continue;
            }
            let still_states = stated
                .entry(source.file.clone())
                .or_insert_with(|| repo::notes_at(root, &head, &source.file))
                .contains(&record.text);
            if still_states {
                continue;
            }
            let changed = self.retract(
                record.id,
                Retraction {
                    file: source.file,
                    commit: head.clone(),
                    at: now(),
                },
                actor.clone(),
            )?;
            retracted += usize::from(changed);
        }
        Ok(retracted)
    }

    /// Curate a record out because the tree stopped stating it. Marked as the
    /// sweep's doing, so the note coming back seeds it again - unlike
    /// [`Self::forget`], which is a person's decision and final.
    fn retract(&self, id: MemoryId, retraction: Retraction, by: Actor) -> Result<bool> {
        let reason = format!(
            "{} no longer states it at {}",
            retraction.file,
            retraction.commit.chars().take(7).collect::<String>()
        );
        let (_, changed) = self.curate(id, by, |record| {
            let event = EventKind::MemoryForgotten {
                memory: id,
                reason: Some(reason),
            };
            if record.pinned || !record.is_current() {
                return (false, event);
            }
            record.forgotten_at = Some(retraction.at);
            record.retracted = Some(retraction);
            (true, event)
        })?;
        if changed && let Some(index) = &self.index {
            index.forget(id)?;
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retraction_preserves_curation_that_landed_after_the_sweep_read() {
        use aether_store::{Commit, Mutation, SqliteStore, WriteGate};
        use std::sync::Arc;
        let s = MemoryService::new(
            Arc::new(SqliteStore::open_in_memory().expect("store")),
            WriteGate::new(),
        );
        for state in ["pinned", "forgotten", "superseded"] {
            let selected = MemoryRecord::from_candidate(
                MemoryCandidate {
                    scope: MemoryScope::Global,
                    kind: aether_core::memory::MemoryKind::Fact,
                    text: "The repository states this durable build convention".into(),
                    structured: None,
                    importance: 0.7,
                    confidence: 0.9,
                    tags: vec![],
                    provenance: Default::default(),
                    supersedes: None,
                },
                now(),
            );
            s.store
                .commit(Commit::new().mutate(Mutation::PutMemory(selected.clone())))
                .unwrap();
            match state {
                "pinned" => {
                    s.pin(selected.id, true, Actor::user()).unwrap();
                }
                "forgotten" => {
                    s.forget(selected.id, Some("operator decision".into()), Actor::user())
                        .unwrap();
                }
                "superseded" => {
                    let mut record = selected.clone();
                    record.superseded_by = Some(MemoryId::new());
                    s.store
                        .commit(Commit::new().mutate(Mutation::PutMemory(record)))
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let before = s.store.memory(selected.id).unwrap().unwrap();
            let events_before = s.store.events(0, 100, &Default::default()).unwrap().len();
            let changed = s
                .retract(
                    selected.id,
                    Retraction {
                        file: ".aether/knowledge/build.md".into(),
                        commit: "abc1234".into(),
                        at: now(),
                    },
                    Actor::System,
                )
                .unwrap();
            assert!(!changed);
            assert_eq!(
                s.store.memory(selected.id).unwrap().unwrap(),
                before,
                "{state}"
            );
            assert_eq!(
                s.store.events(0, 100, &Default::default()).unwrap().len(),
                events_before
            );
        }
    }
}
