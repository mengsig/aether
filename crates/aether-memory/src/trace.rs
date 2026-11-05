//! The trace a turn records of what retrieval did: every memory within the
//! limit with its fate, and the best of the two tails past it.

use std::collections::HashSet;

use aether_core::id::MemoryId;
use aether_core::memory_v2::{
    RetrievalTrace, TRACE_TAIL, TraceCandidate, TraceOmitted, TraceOutcome, TracePolicy,
};

use crate::{Considered, Holdout, Ranking, RetrievalPolicy};

/// The trace of `ranking`, of which `fits` survived the memory budget and
/// `split` is the holdout drawn from those.
pub fn retrieval_trace(
    ranking: &Ranking,
    fits: &[MemoryId],
    split: &Holdout,
    policy: &RetrievalPolicy,
) -> RetrievalTrace {
    assert_eq!(ranking.retrieved.len(), ranking.ranked.len());
    debug_assert_eq!(split.injected.len() + split.held_out.len(), fits.len());
    let fits: HashSet<MemoryId> = fits.iter().copied().collect();
    let held: HashSet<MemoryId> = split.held_out.iter().map(|m| m.record.id).collect();
    let mut candidates: Vec<TraceCandidate> = ranking
        .ranked
        .iter()
        .enumerate()
        .map(|(at, seen)| {
            let outcome = if !fits.contains(&seen.memory) {
                TraceOutcome::Trimmed
            } else if held.contains(&seen.memory) {
                TraceOutcome::HeldOut
            } else {
                TraceOutcome::Injected
            };
            candidate(seen, outcome, Some(at + 1))
        })
        .collect();
    let ranked = candidates.len();
    candidates.extend(
        ranking
            .over_limit
            .iter()
            .take(TRACE_TAIL)
            .enumerate()
            .map(|(at, seen)| candidate(seen, TraceOutcome::OverLimit, Some(ranked + at + 1))),
    );
    let mut cut: Vec<&Considered> = ranking.cut.iter().collect();
    cut.sort_by(|a, b| b.relevance.total_cmp(&a.relevance));
    candidates.extend(
        cut.iter()
            .take(TRACE_TAIL)
            .map(|seen| candidate(seen, TraceOutcome::Cut, None)),
    );
    let omitted = TraceOmitted {
        over_limit: ranking.over_limit.len().saturating_sub(TRACE_TAIL) as u32,
        cut: ranking.cut.len().saturating_sub(TRACE_TAIL) as u32,
    };
    debug_assert!(candidates.len() <= ranked + 2 * TRACE_TAIL);
    RetrievalTrace {
        policy: TracePolicy {
            max_items: policy.max_items as u32,
            min_score: ranking.cutoff,
            holdout_rate: policy.holdout_rate,
        },
        pool: ranking.pool as u32,
        candidates,
        omitted,
    }
}

fn candidate(seen: &Considered, outcome: TraceOutcome, rank: Option<usize>) -> TraceCandidate {
    debug_assert_eq!(seen.score.is_none(), outcome == TraceOutcome::Cut);
    TraceCandidate {
        memory: seen.memory,
        outcome,
        rank: rank.map(|r| r as u32),
        score: seen.score,
        relevance: seen.relevance,
        stale: seen.stale,
        pinned: seen.pinned,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(score: Option<f32>, relevance: f32) -> Considered {
        Considered {
            memory: MemoryId::new(),
            relevance,
            score,
            stale: false,
            pinned: false,
        }
    }

    #[test]
    fn each_fate_is_named_and_the_tails_are_capped_and_counted() {
        let ranked: Vec<Considered> = (0..3).map(|i| seen(Some(3.0 - i as f32), 0.5)).collect();
        let over: Vec<Considered> = (0..20).map(|_| seen(Some(0.1), 0.2)).collect();
        let cut: Vec<Considered> = (0..18).map(|i| seen(None, i as f32 / 100.0)).collect();
        let ranking = Ranking {
            retrieved: Vec::new(),
            ranked: ranked.clone(),
            over_limit: over,
            cut,
            pool: 41,
            cutoff: Some(0.05),
        };
        // The ranking's records are not read: only ids and fits decide.
        let ranking = Ranking {
            retrieved: ranked
                .iter()
                .map(|_| crate::Retrieved {
                    record: test_record(),
                    score: 0.0,
                })
                .collect(),
            ..ranking
        };
        let mut held = crate::Retrieved {
            record: test_record(),
            score: 0.0,
        };
        held.record.id = ranked[1].memory;
        let mut shown = held.clone();
        shown.record.id = ranked[0].memory;
        let split = Holdout {
            injected: vec![shown],
            held_out: vec![held],
            trace: None,
        };
        let trace = retrieval_trace(
            &ranking,
            &[ranked[0].memory, ranked[1].memory],
            &split,
            &RetrievalPolicy::default(),
        );
        let outcomes: Vec<TraceOutcome> = trace.candidates.iter().map(|c| c.outcome).collect();
        assert_eq!(
            outcomes[..3],
            [
                TraceOutcome::Injected,
                TraceOutcome::HeldOut,
                TraceOutcome::Trimmed
            ]
        );
        assert_eq!(trace.candidates.len(), 3 + 2 * TRACE_TAIL);
        assert_eq!(
            trace.omitted,
            TraceOmitted {
                over_limit: 4,
                cut: 2
            }
        );
        let first_cut = &trace.candidates[3 + TRACE_TAIL];
        assert_eq!((first_cut.rank, first_cut.relevance), (None, 0.17));
        assert_eq!(trace.policy.min_score, Some(0.05));
    }

    fn test_record() -> aether_core::memory::MemoryRecord {
        aether_core::memory::MemoryRecord::from_candidate(
            aether_core::memory::MemoryCandidate {
                scope: aether_core::memory::MemoryScope::Global,
                kind: aether_core::memory::MemoryKind::Fact,
                text: "x".into(),
                structured: None,
                importance: 0.5,
                confidence: 0.5,
                tags: Vec::new(),
                provenance: Default::default(),
                supersedes: None,
            },
            aether_core::time::now(),
        )
    }
}
