use crate::{MAX_LINEAGE, MemoryError, MemoryService, Result, RetrievalRequest, Retrieved, tokens};
use aether_core::{
    memory::{MemoryRecord, MemoryScope, role_families},
    time::{Timestamp, now},
};
use aether_store::MemoryQuery;
use std::collections::BTreeSet;

impl MemoryService {
    /// Rank memories for a request. Structural signals dominate; lexical
    /// match narrows the candidate set when a query is given. A repository
    /// named by an old alias is read where its memories now live.
    pub fn retrieve(&self, req: &RetrievalRequest) -> Result<Vec<Retrieved>> {
        self.read_resolved(req.scope.clone(), |scope| {
            self.retrieve_resolved(&RetrievalRequest {
                scope,
                ..req.clone()
            })
        })
    }

    fn retrieve_resolved(&self, req: &RetrievalRequest) -> Result<Vec<Retrieved>> {
        let limit = if req.limit == 0 { 5 } else { req.limit };
        let mut pool = Vec::new();
        let mut seen = BTreeSet::new();
        let mut push_all = |records: Vec<MemoryRecord>, pool: &mut Vec<MemoryRecord>| {
            for r in records {
                if seen.insert(r.id) {
                    pool.push(r);
                }
            }
        };
        let scopes = self.scopes_for(req);
        for scope in &scopes {
            // A page per scope, so one busy scope cannot crowd another out of
            // the pool. Each page also carries the globals, ranked with its own.
            let structural = self.store.memories(&MemoryQuery {
                scope: Some(scope.clone()),
                kinds: req.kinds.clone(),
                limit: limit * 4,
                ..Default::default()
            })?;
            push_all(structural, &mut pool);
            if let Some(q) = req.query.as_deref().filter(|q| !q.trim().is_empty()) {
                let lexical = self.store.memories(&MemoryQuery {
                    scope: Some(scope.clone()),
                    kinds: req.kinds.clone(),
                    text: Some(q.to_owned()),
                    limit: limit * 4,
                    ..Default::default()
                })?;
                push_all(lexical, &mut pool);
            }
        }
        if scopes.is_empty() {
            // Same pair as a scoped query: the structural page first, then the
            // lexical one. Without the structural page the pool is whatever
            // full-text matched, and a memory that shares no whole word with
            // the query - which is exactly what a semantic pass is for - could
            // never be a candidate at all.
            push_all(
                self.store.memories(&MemoryQuery {
                    kinds: req.kinds.clone(),
                    limit: limit * 4,
                    ..Default::default()
                })?,
                &mut pool,
            );
            if let Some(q) = req.query.as_deref().filter(|q| !q.trim().is_empty()) {
                push_all(
                    self.store.memories(&MemoryQuery {
                        kinds: req.kinds.clone(),
                        text: Some(q.to_owned()),
                        limit: limit * 4,
                        ..Default::default()
                    })?,
                    &mut pool,
                );
            }
        }
        let query_tokens = req.query.as_deref().map(tokens).unwrap_or_default();
        let at = now();
        // One more signal, when an index is configured: how close each memory
        // is in meaning. It never replaces the structural and lexical score,
        // it is added to it - a semantic near-miss must not outrank a memory
        // that is actually in scope.
        let semantic = match (&self.index, req.query.as_deref()) {
            (Some(index), Some(q)) if !q.trim().is_empty() => {
                let scores = index.similarity(q, &pool)?;
                // `zip` would truncate the pool to the shorter side, so an
                // index that returned a partial answer would silently drop
                // candidates that are in scope.
                if scores.len() != pool.len() {
                    return Err(MemoryError::Index {
                        index: index.name().to_owned(),
                        detail: format!(
                            "{} score(s) for {} record(s); one per record is the contract",
                            scores.len(),
                            pool.len()
                        ),
                    });
                }
                scores
            }
            _ => vec![0.0; pool.len()],
        };
        let mut scored: Vec<Retrieved> = pool
            .into_iter()
            .zip(semantic)
            .map(|(record, semantic)| {
                let score =
                    score(&record, req, &scopes, &query_tokens, at) + SEMANTIC_WEIGHT * semantic;
                Retrieved { record, score }
            })
            .collect();
        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        scored.truncate(limit);
        Ok(scored)
    }

    /// Every scope the request can see besides `Global`, in the order they
    /// are asked: its repository, that repository's notes for each of its
    /// role families (most specific first), its role, itself, then the
    /// subtrees it sits in, nearest first. Bounded by
    /// [`aether_core::memory::MAX_ROLE_SEGMENTS`] families and [`MAX_LINEAGE`] ancestors.
    fn scopes_for(&self, req: &RetrievalRequest) -> Vec<MemoryScope> {
        let mut scopes = Vec::new();
        if let Some(s) = &req.scope {
            scopes.push(s.clone());
        }
        if let (Some(MemoryScope::Repository { id }), Some(role)) = (&req.scope, &req.role) {
            scopes.extend(role_families(role).into_iter().map(|family| {
                MemoryScope::RepositoryRole {
                    id: id.clone(),
                    role: family.to_owned(),
                }
            }));
        }
        if let Some(role) = &req.role {
            scopes.push(MemoryScope::Role { role: role.clone() });
        }
        if let Some(agent) = req.agent {
            scopes.push(MemoryScope::Agent { id: agent });
            scopes.push(MemoryScope::Subtree { root: agent });
            scopes.extend(
                req.ancestors
                    .iter()
                    .take(MAX_LINEAGE)
                    .map(|&root| MemoryScope::Subtree { root }),
            );
        }
        scopes
    }
}

/// Weight of the semantic pass, on the same scale as the lexical one (0.2):
/// a meaning match is worth about as much as a word match, and neither is
/// worth as much as being in scope.
const SEMANTIC_WEIGHT: f32 = 0.25;

/// `visible` is what [`MemoryService::scopes_for`] asked for this request.
fn score(
    record: &MemoryRecord,
    req: &RetrievalRequest,
    visible: &[MemoryScope],
    query_tokens: &BTreeSet<String>,
    at: Timestamp,
) -> f32 {
    let scope_score = match (&record.scope, &req.scope) {
        (MemoryScope::Global, _) => 0.15,
        (s, Some(q)) if s == q => 0.3,
        (MemoryScope::Role { role }, _) if req.role.as_deref() == Some(role.as_str()) => 0.25,
        (MemoryScope::Agent { id }, _) if req.agent == Some(*id) => 0.25,
        // In the repository and for this role: it matches on both.
        (s @ MemoryScope::RepositoryRole { .. }, _) if visible.contains(s) => 0.35,
        (s @ MemoryScope::Subtree { .. }, _) if visible.contains(s) => 0.25,
        _ => 0.0,
    };
    let age_days = (at - record.created_at).num_seconds().max(0) as f32 / 86_400.0;
    let recency = 0.1 * (1.0 / (1.0 + age_days / 30.0));
    let tag_overlap = if req.tags.is_empty() {
        0.0
    } else {
        let hits = record.tags.iter().filter(|t| req.tags.contains(t)).count() as f32;
        0.15 * (hits / req.tags.len() as f32)
    };
    let lexical = if query_tokens.is_empty() {
        0.0
    } else {
        let rt = tokens(&record.text);
        let hits = rt.intersection(query_tokens).count() as f32;
        0.2 * (hits / query_tokens.len() as f32).min(1.0)
    };
    // A pinned memory is a person's instruction, not a guess: it outranks
    // every signal below rather than nudging them.
    let pinned = if record.pinned { 1.0 } else { 0.0 };
    pinned
        + 0.3 * record.importance
        + 0.1 * record.confidence
        + scope_score
        + recency
        + tag_overlap
        + lexical
}
