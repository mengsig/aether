//! Operator evidence about memories a task actually saw, and the bounded
//! factor by which it moves a memory's recall rank.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::event::Actor;
use crate::id::{InvocationId, MemoryFeedbackId, MemoryId, TaskId};
use crate::time::Timestamp;

pub const MAX_FEEDBACK_REASON_CHARS: usize = 4096;
pub const MEMORY_FEEDBACK_LIMIT: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryVerdict {
    Helped,
    Hurt,
    Neutral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryEvidence {
    Operator,
    Review,
    Outcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryFeedbackSpec {
    pub memory: MemoryId,
    pub verdict: MemoryVerdict,
    pub evidence: MemoryEvidence,
    pub task: TaskId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation: Option<InvocationId>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryFeedback {
    pub id: MemoryFeedbackId,
    #[serde(flatten)]
    pub spec: MemoryFeedbackSpec,
    pub actor: Actor,
    pub created_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retracted_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retraction_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MemoryFeedbackList {
    pub records: Vec<MemoryFeedback>,
    pub truncated: bool,
    /// Absent from an older daemon, whose feedback never moved recall.
    #[serde(default)]
    pub ranking: FeedbackRanking,
}

/// How many tasks, most recent first, can move one memory's recall rank.
pub const RANKING_WINDOW: usize = 20;
/// Helped plus hurt verdicts a memory needs before its rank moves at all.
pub const RANKING_MIN_VOTES: u32 = 2;

/// Evidence that can move recall ranking. Outcome evidence weighs nothing yet.
pub const RANKED_EVIDENCE: [MemoryEvidence; 2] = [MemoryEvidence::Operator, MemoryEvidence::Review];

impl MemoryEvidence {
    pub fn counts_for_ranking(self) -> bool {
        RANKED_EVIDENCE.contains(&self)
    }
}

/// Counted verdicts on one memory: among the latest unretracted operator or
/// review verdict of each task, the [`RANKING_WINDOW`] most recent ones.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeedbackTally {
    pub helped: u32,
    pub hurt: u32,
}

impl FeedbackTally {
    /// The rule over one memory's records, in any order. The store computes
    /// the same tally in one query; its tests hold the two together.
    pub fn of(records: &[MemoryFeedback]) -> Self {
        debug_assert!(
            records
                .windows(2)
                .all(|pair| pair[0].spec.memory == pair[1].spec.memory),
            "a tally is over one memory"
        );
        let mut counted: Vec<&MemoryFeedback> = records
            .iter()
            .filter(|r| r.retracted_at.is_none() && r.spec.evidence.counts_for_ranking())
            .collect();
        counted.sort_by_key(|r| std::cmp::Reverse((r.created_at, r.id)));
        let mut tasks = BTreeSet::new();
        let latest_per_task = counted
            .into_iter()
            .filter(|r| tasks.insert(r.spec.task))
            .take(RANKING_WINDOW);
        let mut tally = Self::default();
        for record in latest_per_task {
            match record.spec.verdict {
                MemoryVerdict::Helped => tally.helped += 1,
                MemoryVerdict::Hurt => tally.hurt += 1,
                MemoryVerdict::Neutral => {}
            }
        }
        debug_assert!((tally.helped + tally.hurt) as usize <= RANKING_WINDOW);
        tally
    }

    pub fn is_active(self) -> bool {
        self.helped + self.hurt >= RANKING_MIN_VOTES
    }

    /// What a non-pinned memory's recall score is multiplied by: exactly 1.0
    /// until the tally is active, then always within [0.75, 1.25].
    pub fn factor(self) -> f32 {
        debug_assert!((self.helped + self.hurt) as usize <= RANKING_WINDOW);
        if !self.is_active() {
            return 1.0;
        }
        let factor = 0.75 + 0.5 * (self.helped + 1) as f32 / (self.helped + self.hurt + 2) as f32;
        debug_assert!((0.75..=1.25).contains(&factor), "factor {factor}");
        debug_assert_eq!(factor > 1.0, self.helped > self.hurt);
        factor
    }
}

/// A memory's current tally and the factor it gives, as the host reports it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FeedbackRanking {
    pub helped: u32,
    pub hurt: u32,
    /// 1.0 until two verdicts are helped or hurt, then 0.75 to 1.25.
    pub factor: f32,
}

impl Default for FeedbackRanking {
    fn default() -> Self {
        FeedbackTally::default().into()
    }
}

impl From<FeedbackTally> for FeedbackRanking {
    fn from(tally: FeedbackTally) -> Self {
        Self {
            helped: tally.helped,
            hurt: tally.hurt,
            factor: tally.factor(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests;
