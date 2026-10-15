use crate::{MAX_LINEAGE, MemoryError, MemoryService, Result, RetrievalRequest, Retrieved, tokens};
use aether_core::{
    id::MemoryId,
    memory::{MemoryRecord, MemoryScope, role_families},
    memory_feedback::FeedbackTally,
    time::{Timestamp, now},
};
use aether_store::MemoryQuery;
use std::collections::{BTreeMap, BTreeSet};

impl MemoryService {
    /// Rank memories for a request. Structural signals dominate; lexical
    /// match narrows the candidate set when a query is given, and counted
    /// feedback scales a non-pinned memory's score by 0.75 to 1.25. A
    /// repository named by an old alias is read where its memories now live.
    pub fn retrieve(&self, req: &RetrievalRequest) -> Result<Vec<Retrieved>> {
        let limit = if req.limit == 0 { 5 } else { req.limit };
        // Only collection is repeated when an alias lands mid-read; ranking,
        // and its one feedback read, run once on the pool that was kept.
        let (req, scopes, pool) = self.read_resolved(req.scope.clone(), |scope| {
            let req = RetrievalRequest {
                scope,
                ..req.clone()
            };
            let scopes = self.scopes_for(&req);
            let pool = self.candidate_pool(&req, &scopes, limit)?;
            Ok((req, scopes, pool))
        })?;
        self.rank(&req, &scopes, pool, limit)
    }

    fn rank(
        &self,
        req: &RetrievalRequest,
        scopes: &[MemoryScope],
        pool: Vec<MemoryRecord>,
        limit: usize,
    ) -> Result<Vec<Retrieved>> {
        debug_assert!(limit > 0);
        let query_tokens = req.query.as_deref().map(tokens).unwrap_or_default();
        let at = now();
        let semantic = self.semantic_scores(req, &pool)?;
        let tallies = self.feedback_tallies(&pool)?;
        let mut scored: Vec<Retrieved> = pool
            .into_iter()
            .zip(semantic)
            .map(|(record, semantic)| {
                let score =
                    score(&record, req, scopes, &query_tokens, at) + SEMANTIC_WEIGHT * semantic;
                let score = with_feedback(score, &record, tallies.get(&record.id));
                Retrieved { record, score }
            })
            .collect();
        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        scored.truncate(limit);
        debug_assert!(scored.len() <= limit);
        debug_assert!(scored.windows(2).all(|w| w[0].score >= w[1].score));
        Ok(scored)
    }

    /// Every candidate the request can see, each once: a structural page and,
    /// with a query, a lexical page per scope.
    fn candidate_pool(
        &self,
        req: &RetrievalRequest,
        scopes: &[MemoryScope],
        limit: usize,
    ) -> Result<Vec<MemoryRecord>> {
        debug_assert!(limit > 0);
        let query = req.query.as_deref().filter(|q| !q.trim().is_empty());
        let mut pool = Vec::new();
        let mut seen = BTreeSet::new();
        let mut push_all = |records: Vec<MemoryRecord>, pool: &mut Vec<MemoryRecord>| {
            for r in records {
                if seen.insert(r.id) {
                    pool.push(r);
                }
            }
        };
        // A page per scope, so one busy scope cannot crowd another out; each
        // carries the globals. No scope still reads the structural page first.
        let pages: Vec<Option<MemoryScope>> = if scopes.is_empty() {
            vec![None]
        } else {
            scopes.iter().cloned().map(Some).collect()
        };
        for scope in pages {
            let page = |text: Option<&str>| MemoryQuery {
                scope: scope.clone(),
                kinds: req.kinds.clone(),
                text: text.map(str::to_owned),
                limit: limit * 4,
                ..Default::default()
            };
            push_all(self.store.memories(&page(None))?, &mut pool);
            if let Some(q) = query {
                push_all(self.store.memories(&page(Some(q)))?, &mut pool);
            }
        }
        debug_assert_eq!(pool.len(), seen.len());
        Ok(pool)
    }

    /// How close each memory is in meaning, when an index is configured. It is
    /// added to the structural score, so a near-miss cannot outrank scope.
    fn semantic_scores(&self, req: &RetrievalRequest, pool: &[MemoryRecord]) -> Result<Vec<f32>> {
        let (Some(index), Some(q)) = (&self.index, req.query.as_deref()) else {
            return Ok(vec![0.0; pool.len()]);
        };
        if q.trim().is_empty() {
            return Ok(vec![0.0; pool.len()]);
        }
        let scores = index.similarity(q, pool)?;
        // `zip` would truncate the pool, silently dropping in-scope candidates.
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
        Ok(scores)
    }

    /// Counted feedback on the pool, in one read, and none for an empty pool.
    /// A store failure fails the retrieval: it is never read as no feedback.
    fn feedback_tallies(&self, pool: &[MemoryRecord]) -> Result<BTreeMap<MemoryId, FeedbackTally>> {
        if pool.is_empty() {
            return Ok(BTreeMap::new());
        }
        let ids: Vec<MemoryId> = pool.iter().map(|record| record.id).collect();
        let tallies = self.store.memory_feedback_tallies(&ids)?;
        debug_assert!(tallies.len() <= ids.len());
        debug_assert!(
            tallies.keys().all(|id| ids.contains(id)),
            "a tally outside the pool"
        );
        Ok(tallies)
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

/// Feedback reorders and never filters. A pinned memory, and one without an
/// active tally, keeps its score bit for bit.
fn with_feedback(score: f32, record: &MemoryRecord, tally: Option<&FeedbackTally>) -> f32 {
    debug_assert!(score.is_finite(), "score {score}");
    let Some(tally) = tally.filter(|t| t.is_active() && !record.pinned) else {
        return score;
    };
    let moved = score * tally.factor();
    debug_assert!((moved - score).abs() <= 0.25 * score.abs());
    moved
}

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
