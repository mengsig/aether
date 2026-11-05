//! `memory_retrieval_explain`: the trace each snapshot recorded when its
//! turn was built, reconciled with the memory layer the model was given.

use aether_core::event::Actor;
use aether_core::id::{InvocationId, SnapshotId, TaskId};
use aether_core::memory_v2::{RetrievalTrace, TraceOutcome};
use aether_core::memory_v2_reports::{ExplainedTurn, RetrievalExplain};
use aether_core::snapshot::SnapshotMemories;

use crate::{Result, Runtime, RuntimeError};

impl Runtime {
    /// What retrieval did for every model call of `task`, or the one
    /// `invocation`; exactly one must be named. Operators only.
    pub fn memory_retrieval_explain(
        &self,
        task: Option<TaskId>,
        invocation: Option<InvocationId>,
        by: &Actor,
    ) -> Result<RetrievalExplain> {
        super::require_operator(by, "explain memory retrieval")?;
        let context = self.memories_in_context(task, invocation, by)?;
        let ids: Vec<SnapshotId> = context.snapshots.iter().map(|s| s.snapshot).collect();
        let traces = self.store.snapshot_retrievals(&ids)?;
        if traces.len() != ids.len() {
            let missing = ids
                .iter()
                .find(|id| !traces.iter().any(|(found, _)| found == *id));
            return Err(RuntimeError::NotFound(format!(
                "snapshot {}",
                missing.map_or_else(String::new, ToString::to_string)
            )));
        }
        let turns: Vec<ExplainedTurn> = context
            .snapshots
            .into_iter()
            .zip(traces)
            .map(|(entry, (id, trace))| {
                assert_eq!(entry.snapshot, id, "traces come back in the order asked");
                explained_turn(entry, trace)
            })
            .collect();
        debug_assert_eq!(turns.len(), ids.len());
        Ok(RetrievalExplain {
            turns,
            truncated: context.truncated,
        })
    }
}

/// One snapshot's turn. `injected` is the snapshot's own memory layer, so a
/// trace entry marked for injection that the layer lacks is reported trimmed.
fn explained_turn(entry: SnapshotMemories, trace: Option<RetrievalTrace>) -> ExplainedTurn {
    let mut turn = ExplainedTurn {
        snapshot: entry.snapshot,
        agent: entry.agent,
        task: entry.task,
        turn: entry.turn,
        at: entry.at,
        invocations: entry.invocations,
        traced: trace.is_some(),
        injected: entry.memories,
        policy: None,
        pool: 0,
        candidates: Vec::new(),
        omitted: Default::default(),
    };
    let Some(trace) = trace else {
        return turn;
    };
    debug_assert!(trace.candidates.len() <= trace.pool as usize);
    turn.policy = Some(trace.policy);
    turn.pool = trace.pool;
    turn.omitted = trace.omitted;
    turn.candidates = trace
        .candidates
        .into_iter()
        .map(|mut candidate| {
            if candidate.outcome == TraceOutcome::Injected
                && !turn.injected.contains(&candidate.memory)
            {
                candidate.outcome = TraceOutcome::Trimmed;
            }
            candidate
        })
        .collect();
    turn
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::id::{AgentId, MemoryId};
    use aether_core::memory_v2::{TraceCandidate, TracePolicy};

    fn candidate(memory: MemoryId, outcome: TraceOutcome) -> TraceCandidate {
        TraceCandidate {
            memory,
            outcome,
            rank: Some(1),
            score: Some(0.5),
            relevance: 0.5,
            stale: false,
            pinned: false,
        }
    }

    #[test]
    fn an_injection_the_layer_lacks_is_reported_trimmed_and_no_trace_is_untraced() {
        let (shown, dropped, held) = (MemoryId::new(), MemoryId::new(), MemoryId::new());
        let entry = SnapshotMemories {
            snapshot: SnapshotId::new(),
            invocations: Vec::new(),
            agent: AgentId::new(),
            task: None,
            turn: 1,
            at: aether_core::time::now(),
            memories: vec![shown],
        };
        let trace = RetrievalTrace {
            policy: TracePolicy {
                max_items: 6,
                min_score: None,
                holdout_rate: 0.05,
            },
            pool: 3,
            candidates: vec![
                candidate(shown, TraceOutcome::Injected),
                candidate(dropped, TraceOutcome::Injected),
                candidate(held, TraceOutcome::HeldOut),
            ],
            omitted: Default::default(),
        };
        let turn = explained_turn(entry.clone(), Some(trace));
        let outcomes: Vec<_> = turn.candidates.iter().map(|c| c.outcome).collect();
        assert_eq!(
            outcomes,
            [
                TraceOutcome::Injected,
                TraceOutcome::Trimmed,
                TraceOutcome::HeldOut
            ]
        );
        assert!(turn.traced);
        let untraced = explained_turn(entry, None);
        assert!(!untraced.traced && untraced.candidates.is_empty());
        assert_eq!(untraced.injected, vec![shown]);
    }
}
