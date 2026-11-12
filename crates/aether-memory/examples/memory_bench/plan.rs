//! The plan `evals/memory_bench.py` writes: the dataset flattened into
//! candidates in admission order and the queries to rank them for.

use std::collections::BTreeSet;
use std::path::Path;

use aether_core::memory::{MemoryKind, MemoryScope};
use serde::Deserialize;

use crate::BenchResult;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    /// The semantic pass: `none` (the shipped default) or `local`.
    pub index: String,
    /// Cap on the memory layer, in estimated tokens: the largest
    /// `ContextBudget::memory_max` any model gets.
    pub memory_max: u32,
    pub memories: Vec<PlanMemory>,
    pub queries: Vec<PlanQuery>,
    pub latency: LatencyPlan,
    /// Consolidate near-duplicates after seeding, as `consolidate_memories` does.
    #[serde(default)]
    pub consolidate: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanMemory {
    pub key: String,
    pub scope: MemoryScope,
    pub kind: MemoryKind,
    pub text: String,
    pub importance: f32,
    pub confidence: f32,
    #[serde(default)]
    pub tags: Vec<String>,
    /// How long before the run it was recorded; recency is a ranking signal.
    #[serde(default)]
    pub age_days: f32,
    /// The key of an earlier memory this one names as replaced.
    #[serde(default)]
    pub supersedes: Option<String>,
    /// Recorded by a person rather than by an agent's `remember`.
    #[serde(default)]
    pub operator: bool,
    #[serde(default)]
    pub feedback: FeedbackPlan,
}

/// Counted review verdicts, one task each, as feedback ranking counts them.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedbackPlan {
    #[serde(default)]
    pub helped: u32,
    #[serde(default)]
    pub hurt: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanQuery {
    pub id: String,
    /// The repository the agent works in; `None` for an agent outside one.
    pub repository: Option<String>,
    pub role: String,
    /// What a turn ranks on: the task objective and the new messages.
    pub query: String,
    /// What a turn asks for: six (`aether-runtime` `retrieve_memories`).
    pub limit: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LatencyPlan {
    /// Timed retrievals per query.
    pub reps: usize,
    /// Synthetic memories added to the plan's in the scaled store.
    pub filler: usize,
}

impl Plan {
    pub fn read(path: &Path) -> BenchResult<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("read plan {}: {e}", path.display()))?;
        let plan: Plan = serde_json::from_str(&text)
            .map_err(|e| format!("parse plan {}: {e}", path.display()))?;
        plan.validate()?;
        Ok(plan)
    }

    /// Mistakes in the dataset, caught before anything is seeded.
    pub fn validate(&self) -> BenchResult<()> {
        let mut keys = BTreeSet::new();
        for memory in &self.memories {
            if let Some(old) = &memory.supersedes
                && !keys.contains(old.as_str())
            {
                return Err(format!(
                    "{} supersedes {old}, which comes later or not at all",
                    memory.key
                )
                .into());
            }
            if !keys.insert(memory.key.as_str()) {
                return Err(format!("memory key {} appears twice", memory.key).into());
            }
            if !(memory.age_days.is_finite() && memory.age_days >= 0.0) {
                return Err(format!("{}: age_days must be >= 0", memory.key).into());
            }
        }
        let ids: BTreeSet<&str> = self.queries.iter().map(|q| q.id.as_str()).collect();
        if ids.len() != self.queries.len() {
            return Err("query ids must be unique".into());
        }
        if self.queries.iter().any(|q| q.limit == 0) || self.latency.reps == 0 {
            return Err("query limits and latency reps must be positive".into());
        }
        Ok(())
    }
}
