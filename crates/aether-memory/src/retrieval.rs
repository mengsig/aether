use crate::{MAX_LINEAGE, MemoryError, MemoryService, Result, RetrievalRequest, Retrieved, tokens};
use aether_core::{
    id::MemoryId,
    memory::{MemoryRecord, MemoryScope, role_families},
    memory_feedback::FeedbackTally,
    time::{Timestamp, now},
};
use aether_store::{Commit, MemoryQuery, MemoryStaleMark, Mutation};
use std::collections::{BTreeMap, BTreeSet};

/// A ranking and how every candidate in its pool fared.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranking {
    /// What [`MemoryService::retrieve`] returns: at most the limit, best first.
    pub retrieved: Vec<Retrieved>,
    /// How each of `retrieved` was scored, in the same order.
    pub ranked: Vec<Considered>,
    /// Passed the cutoff but ranked past the limit, best first.
    pub over_limit: Vec<Considered>,
    /// Below the minimum relevance, in pool order.
    pub cut: Vec<Considered>,
    /// Candidates considered.
    pub pool: usize,
    /// The minimum relevance applied; `None` for a request with no query.
    pub cutoff: Option<f32>,
}

/// One candidate as ranking saw it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Considered {
    pub memory: MemoryId,
    pub relevance: f32,
    /// The final score; `None` for a cut candidate, which is never scored.
    pub score: Option<f32>,
    pub stale: bool,
    pub pinned: bool,
}

/// Retrieval policy: holdout and the relevance cutoff are off by default.
/// A cutoff can drop relevant memories that share no word with the query.
#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalPolicy {
    /// Most memories one turn is shown.
    pub max_items: usize,
    /// Least relevance, in `0..=1`, a memory needs to a request that has a
    /// query; 0, the default, cuts nothing. A pinned memory needs none.
    pub min_score: f32,
    /// Share of eligible injections withheld and logged, in `0..=1`.
    pub holdout_rate: f32,
    /// Seeds every holdout draw: the same seed, unit and memory always draw
    /// the same. Task ids are random, so a fixed seed still spreads the arms.
    pub holdout_seed: u64,
}

impl Default for RetrievalPolicy {
    fn default() -> Self {
        Self {
            max_items: 6,
            min_score: 0.0,
            holdout_rate: 0.0,
            holdout_seed: 0,
        }
    }
}

impl RetrievalPolicy {
    /// Refuse a policy retrieval cannot run under, naming the bad setting.
    pub fn validate(&self) -> Result<()> {
        if self.max_items == 0 || self.max_items > MAX_ITEMS {
            return Err(MemoryError::Invalid(format!(
                "memory.retrieval.max_items must be 1 to {MAX_ITEMS}, not {}",
                self.max_items
            )));
        }
        if !(0.0..=1.0).contains(&self.min_score) {
            return Err(MemoryError::Invalid(format!(
                "memory.retrieval.min_score must be 0 to 1, not {}",
                self.min_score
            )));
        }
        if !(0.0..=1.0).contains(&self.holdout_rate) {
            return Err(MemoryError::Invalid(format!(
                "memory.retrieval.holdout_rate must be 0 to 1, not {}",
                self.holdout_rate
            )));
        }
        Ok(())
    }
}

/// Past this, the layer's token cap decides long before the count does.
const MAX_ITEMS: usize = 50;

impl MemoryService {
    /// Rank memories for a request. Structural signals dominate; lexical
    /// match narrows the candidate set when a query is given, and counted
    /// feedback scales a non-pinned memory's score by 0.75 to 1.25. With a
    /// query, a memory that does not reach [`RetrievalPolicy::min_score`]
    /// relevance is cut, unless pinned, so an unrelated task gets fewer
    /// memories rather than always `limit`; a stale-marked one is demoted. A
    /// repository named by an old alias is read where its memories now live.
    pub fn retrieve(&self, req: &RetrievalRequest) -> Result<Vec<Retrieved>> {
        Ok(self.retrieve_traced(req)?.retrieved)
    }

    /// [`Self::retrieve`], also saying how each candidate fared, for the
    /// turn's trace. The ranking is the same.
    pub fn retrieve_traced(&self, req: &RetrievalRequest) -> Result<Ranking> {
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
    ) -> Result<Ranking> {
        debug_assert!(limit > 0);
        let query_tokens = req.query.as_deref().map(tokens).unwrap_or_default();
        // Only a request that says what it is about can be judged irrelevant.
        let cutoff = (!query_tokens.is_empty()).then_some(self.retrieval.min_score);
        let at = now();
        let pooled = pool.len();
        let semantic = self.semantic_scores(req, &pool)?;
        let tallies = self.feedback_tallies(&pool)?;
        let stale = self.stale_marks(&pool)?;
        let mut cut = Vec::new();
        let mut scored: Vec<(Retrieved, Considered)> = Vec::with_capacity(pool.len());
        for (record, semantic) in pool.into_iter().zip(semantic) {
            let fit = Fit::of(&record, req, &query_tokens, semantic);
            let is_stale = stale.contains_key(&record.id);
            let mut seen = Considered {
                memory: record.id,
                relevance: fit.relevance,
                score: None,
                stale: is_stale,
                pinned: record.pinned,
            };
            if cutoff.is_some_and(|min| !record.pinned && fit.relevance < min) {
                cut.push(seen);
                continue;
            }
            let score = score(&record, req, scopes, &fit, at) + SEMANTIC_WEIGHT * semantic;
            let score = with_feedback(score, &record, tallies.get(&record.id));
            let score = with_staleness(score, is_stale);
            seen.score = Some(score);
            scored.push((Retrieved { record, score }, seen));
        }
        // Stable, on the score alone: the order `retrieve` always gave.
        scored.sort_by(|a, b| b.0.score.total_cmp(&a.0.score));
        let over_limit: Vec<Considered> = scored
            .drain(limit.min(scored.len())..)
            .map(|(_, seen)| seen)
            .collect();
        let (retrieved, ranked): (Vec<_>, Vec<_>) = scored.into_iter().unzip();
        debug_assert!(retrieved.len() <= limit);
        debug_assert!(retrieved.windows(2).all(|w| w[0].score >= w[1].score));
        debug_assert_eq!(retrieved.len() + over_limit.len() + cut.len(), pooled);
        Ok(Ranking {
            retrieved,
            ranked,
            over_limit,
            cut,
            pool: pooled,
            cutoff,
        })
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

    /// Mark a memory as possibly stale by hand, with why, or clear any mark
    /// with `None`. Committed at once; returns whether anything changed.
    pub fn set_stale(&self, id: MemoryId, reason: Option<String>) -> Result<bool> {
        if reason.as_deref().is_some_and(|r| r.trim().is_empty()) {
            return Err(MemoryError::Invalid("a stale mark needs a reason".into()));
        }
        let hold = self.gate.hold();
        self.store.memory(id)?.ok_or(MemoryError::NotFound(id))?;
        let marked = self.store.memory_stale_marks(&[id])?.remove(&id);
        debug_assert!(marked.as_ref().is_none_or(|m| m.memory == id));
        let mutation = match (reason, marked) {
            (None, None) => return Ok(false),
            (Some(reason), Some(mark)) if mark.reason == reason => return Ok(false),
            (None, Some(_)) => Mutation::ClearMemoryStale(id),
            (Some(reason), _) => Mutation::MarkMemoryStale(MemoryStaleMark {
                memory: id,
                reason,
                since: now(),
                revision: None,
            }),
        };
        self.commit(&hold, Commit::new().mutate(mutation))?;
        Ok(true)
    }

    /// The sweep's mark: `id`'s source stopped stating it at `revision`. An
    /// existing mark is kept, so the revision a clear must contain stays the
    /// one that first established it. Returns whether a mark was written.
    pub(crate) fn mark_source_departed(
        &self,
        id: MemoryId,
        reason: String,
        revision: &str,
    ) -> Result<bool> {
        debug_assert!(!reason.trim().is_empty());
        debug_assert!(!revision.trim().is_empty());
        let hold = self.gate.hold();
        if self.store.memory_stale_marks(&[id])?.contains_key(&id) {
            return Ok(false);
        }
        let mark = MemoryStaleMark {
            memory: id,
            reason,
            since: now(),
            revision: Some(revision.to_owned()),
        };
        self.commit(&hold, Commit::new().mutate(Mutation::MarkMemoryStale(mark)))?;
        Ok(true)
    }

    /// Clear `id`'s mark only while it is still the sweep's mark set at
    /// `revision`, which the caller checked its history contains. A mark
    /// replaced since, or set by hand, is left alone.
    pub(crate) fn clear_source_departed(&self, id: MemoryId, revision: &str) -> Result<bool> {
        let hold = self.gate.hold();
        let marked = self.store.memory_stale_marks(&[id])?.remove(&id);
        debug_assert!(marked.as_ref().is_none_or(|m| m.memory == id));
        if marked.and_then(|m| m.revision).as_deref() != Some(revision) {
            return Ok(false);
        }
        let clear = Commit::new().mutate(Mutation::ClearMemoryStale(id));
        self.commit(&hold, clear)?;
        Ok(true)
    }

    /// The stale mark of `id`, if any.
    pub(crate) fn stale_mark(&self, id: MemoryId) -> Result<Option<MemoryStaleMark>> {
        let mark = self.store.memory_stale_marks(&[id])?.remove(&id);
        debug_assert!(mark.as_ref().is_none_or(|m| m.memory == id));
        Ok(mark)
    }

    /// Stale marks on the pool, in one read, and none for an empty pool. A
    /// store failure fails the retrieval, as for feedback.
    fn stale_marks(&self, pool: &[MemoryRecord]) -> Result<BTreeMap<MemoryId, MemoryStaleMark>> {
        if pool.is_empty() {
            return Ok(BTreeMap::new());
        }
        let ids: Vec<MemoryId> = pool.iter().map(|record| record.id).collect();
        let marks = self.store.memory_stale_marks(&ids)?;
        debug_assert!(marks.len() <= ids.len());
        debug_assert!(
            marks
                .iter()
                .all(|(id, mark)| *id == mark.memory && ids.contains(id)),
            "a stale mark outside the pool"
        );
        Ok(marks)
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

/// A memory whose source may have moved on still counts, at half its score:
/// it ranks behind what is known to hold, and is never hidden for it.
pub(crate) const STALE_FACTOR: f32 = 0.5;

fn with_staleness(score: f32, stale: bool) -> f32 {
    debug_assert!(score.is_finite(), "score {score}");
    if !stale {
        return score;
    }
    let demoted = score * STALE_FACTOR;
    debug_assert!(demoted.abs() <= score.abs());
    demoted
}

/// Trigram similarity two unrelated sentences reach anyway. Only what is
/// above it is evidence that a memory is about the request.
const SEMANTIC_FLOOR: f32 = 0.4;

/// How well one memory fits the request's own words, apart from where it
/// sits. `relevance` is what the cutoff reads.
struct Fit {
    /// The share of the query's meaningful words the memory states, as the
    /// score has always weighed it.
    coverage: f32,
    /// The best of three signals, each in `0..=1`: shared words as a set
    /// cosine (fair to a long objective), meaning above the floor, and tags.
    relevance: f32,
    tag_overlap: f32,
}

impl Fit {
    fn of(
        record: &MemoryRecord,
        req: &RetrievalRequest,
        query_tokens: &BTreeSet<String>,
        semantic: f32,
    ) -> Self {
        debug_assert!((0.0..=1.0).contains(&semantic), "semantic {semantic}");
        let tag_overlap = if req.tags.is_empty() {
            0.0
        } else {
            let hits = record.tags.iter().filter(|t| req.tags.contains(t)).count() as f32;
            (hits / req.tags.len() as f32).min(1.0)
        };
        let (coverage, cosine) = if query_tokens.is_empty() {
            (0.0, 0.0)
        } else {
            let rt = tokens(&record.text);
            let hits = rt.intersection(query_tokens).count() as f32;
            let both = (rt.len() * query_tokens.len()).max(1) as f32;
            (
                (hits / query_tokens.len() as f32).min(1.0),
                (hits / both.sqrt()).min(1.0),
            )
        };
        let meaning = ((semantic - SEMANTIC_FLOOR) / (1.0 - SEMANTIC_FLOOR)).max(0.0);
        let relevance = cosine.max(meaning).max(tag_overlap);
        debug_assert!((0.0..=1.0).contains(&relevance), "relevance {relevance}");
        Self {
            coverage,
            relevance,
            tag_overlap,
        }
    }
}

/// `visible` is what [`MemoryService::scopes_for`] asked for this request.
fn score(
    record: &MemoryRecord,
    req: &RetrievalRequest,
    visible: &[MemoryScope],
    fit: &Fit,
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
    // A pinned memory is a person's instruction, not a guess: it outranks
    // every signal below rather than nudging them.
    let pinned = if record.pinned { 1.0 } else { 0.0 };
    pinned
        + 0.3 * record.importance
        + 0.1 * record.confidence
        + scope_score
        + recency
        + 0.15 * fit.tag_overlap
        + 0.2 * fit.coverage
}
