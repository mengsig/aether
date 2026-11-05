//! The injection holdout: a share of what retrieval found is withheld from
//! the model and logged, so a memory's lift is measured as a difference
//! between turns that saw it and turns that did not.
//!
//! The draw is a hash of the seed, the unit (the task, or the agent with
//! none) and the memory, never a fresh random number: a memory held out of a
//! task stays held out for every turn of it, so the layer is not resent each
//! turn and the arms do not blur. A pinned memory is a person's instruction
//! and is never withheld.

use aether_core::id::{AgentId, MemoryId, TaskId};
use aether_core::time::Timestamp;
use aether_store::{MemoryHoldout, Mutation};

use crate::{MemoryService, Retrieved};

/// What a turn is shown and what it was drawn to go without.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Holdout {
    pub injected: Vec<Retrieved>,
    pub held_out: Vec<Retrieved>,
}

/// Who the draw is for: one task, or one agent's turns without a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoldoutUnit {
    pub agent: AgentId,
    pub task: Option<TaskId>,
}

impl HoldoutUnit {
    fn key(&self) -> String {
        self.task
            .map_or_else(|| self.agent.to_string(), |task| task.to_string())
    }
}

impl MemoryService {
    /// Split `retrieved` into the injected and the held-out arm at the
    /// configured rate. Order within each arm is the ranking's.
    pub fn hold_out(&self, retrieved: Vec<Retrieved>, unit: HoldoutUnit) -> Holdout {
        let rate = self.retrieval.holdout_rate;
        debug_assert!((0.0..=1.0).contains(&rate), "holdout rate {rate}");
        let found = retrieved.len();
        let key = unit.key();
        let (held_out, injected): (Vec<_>, Vec<_>) = retrieved.into_iter().partition(|item| {
            !item.record.pinned && draw(self.retrieval.holdout_seed, &key, item.record.id) < rate
        });
        debug_assert_eq!(injected.len() + held_out.len(), found);
        debug_assert!(held_out.iter().all(|item| !item.record.pinned));
        Holdout { injected, held_out }
    }

    /// One log row per held-out memory, for the turn's own commit. A memory
    /// already logged for this unit is kept as first logged.
    pub fn holdout_log(
        &self,
        held_out: &[Retrieved],
        unit: HoldoutUnit,
        at: Timestamp,
    ) -> Vec<Mutation> {
        let rate = self.retrieval.holdout_rate;
        let log: Vec<Mutation> = held_out
            .iter()
            .map(|item| {
                Mutation::LogMemoryHoldout(MemoryHoldout {
                    memory: item.record.id,
                    agent: unit.agent,
                    task: unit.task,
                    at,
                    rate,
                    score: item.score,
                })
            })
            .collect();
        debug_assert_eq!(log.len(), held_out.len());
        log
    }
}

/// A uniform draw in `[0, 1)`: FNV-1a over the unit and the memory, mixed
/// with the seed by SplitMix64. Below 1 always, so a rate of 1 holds every
/// eligible memory out and a rate of 0 none.
fn draw(seed: u64, unit: &str, memory: MemoryId) -> f32 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in unit.bytes().chain([0]).chain(memory.to_string().bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    let mut z = hash ^ seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    // 24 bits: exactly representable, so the draw never rounds up to 1.
    let unit_draw = (z >> 40) as f32 / (1u64 << 24) as f32;
    debug_assert!((0.0..1.0).contains(&unit_draw));
    unit_draw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_unit_and_memory_draw_the_same_and_a_new_seed_redraws() {
        let memories: Vec<MemoryId> = (0..64).map(|_| MemoryId::new()).collect();
        let first: Vec<f32> = memories.iter().map(|m| draw(7, "task", *m)).collect();
        let again: Vec<f32> = memories.iter().map(|m| draw(7, "task", *m)).collect();
        let reseeded: Vec<f32> = memories.iter().map(|m| draw(8, "task", *m)).collect();
        let other_unit: Vec<f32> = memories.iter().map(|m| draw(7, "other", *m)).collect();
        assert_eq!(first, again);
        assert_ne!(first, reseeded);
        assert_ne!(first, other_unit);
        assert!(first.iter().all(|d| (0.0..1.0).contains(d)));
    }

    #[test]
    fn draws_spread_over_the_unit_interval() {
        let draws: Vec<f32> = (0..4_000)
            .map(|i| draw(0, &format!("task-{i}"), MemoryId::new()))
            .collect();
        let below = draws.iter().filter(|d| **d < 0.05).count();
        // About 200 of 4,000; a broken mix would put nearly all or none there.
        assert!((100..300).contains(&below), "{below} of 4000 below 0.05");
    }
}
