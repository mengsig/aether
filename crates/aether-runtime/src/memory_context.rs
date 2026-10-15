//! Which stored memories each model call saw, for a host weighing them.
//!
//! Read from the stored snapshots, found through the task's indexed
//! `snapshot.created` and invocation events, or the type-indexed failed
//! calls for one with no invocation row; nothing here scans the whole log.

use std::collections::HashMap;

use aether_core::event::{Actor, EventKind};
use aether_core::id::{InvocationId, SnapshotId, TaskId};
use aether_core::snapshot::{MemoriesInContext, SnapshotMemories};
use aether_store::EventFilter;

use crate::{Result, Runtime, RuntimeError};

/// Most snapshots one answer lists; a longer task reports `truncated`.
pub const MEMORY_CONTEXT_LIMIT: usize = 256;
/// Events read per page while walking a task's model calls.
const PAGE: usize = 512;

impl Runtime {
    /// The memories every model call of `task`, or the one `invocation`, was
    /// given; exactly one must be named. Operators only.
    pub fn memories_in_context(
        &self,
        task: Option<TaskId>,
        invocation: Option<InvocationId>,
        by: &Actor,
    ) -> Result<MemoriesInContext> {
        if !matches!(by, Actor::User { .. }) {
            return Err(RuntimeError::Invalid(
                "only an operator can ask which memories were in context".into(),
            ));
        }
        match (task, invocation) {
            (Some(task), None) => self.task_memories(task),
            (None, Some(invocation)) => self.invocation_memories(invocation),
            _ => Err(RuntimeError::Invalid(
                "name exactly one of task or invocation".into(),
            )),
        }
    }

    fn invocation_memories(&self, id: InvocationId) -> Result<MemoriesInContext> {
        let snapshot = match self.store.invocation(id)? {
            Some(invocation) => invocation.snapshot_id,
            None => self
                .failed_call_snapshot(id)?
                .ok_or_else(|| RuntimeError::NotFound(id.to_string()))?,
        };
        let mut snapshots = self.stored_memories(&[snapshot])?;
        debug_assert_eq!(snapshots.len(), 1);
        snapshots[0].invocations.push(id);
        Ok(MemoriesInContext {
            snapshots,
            truncated: false,
        })
    }

    /// The snapshot a failed call answered from, read from its
    /// `invocation.failed` event: daemon recovery and imports keep no row for it.
    fn failed_call_snapshot(&self, id: InvocationId) -> Result<Option<SnapshotId>> {
        let filter = EventFilter {
            types: vec!["invocation.failed".into()],
            ..Default::default()
        };
        let mut after = 0;
        loop {
            let page = self.store.events(after, PAGE, &filter)?;
            for event in &page {
                debug_assert!(event.seq > after, "events arrive in sequence order");
                after = event.seq;
                if let EventKind::InvocationFailed {
                    invocation,
                    snapshot,
                    ..
                } = &event.kind
                    && *invocation == id
                {
                    return Ok(Some(*snapshot));
                }
            }
            if page.len() < PAGE {
                return Ok(None);
            }
        }
    }

    fn task_memories(&self, task: TaskId) -> Result<MemoriesInContext> {
        self.store
            .task(task)?
            .ok_or_else(|| RuntimeError::NotFound(task.to_string()))?;
        let (calls, truncated) = self.task_model_calls(task)?;
        let ids: Vec<SnapshotId> = calls.calls.iter().map(|(id, _)| *id).collect();
        let mut snapshots = self.stored_memories(&ids)?;
        for (entry, (id, invocations)) in snapshots.iter_mut().zip(calls.calls) {
            debug_assert_eq!(entry.snapshot, id);
            entry.invocations = invocations;
        }
        Ok(MemoriesInContext {
            snapshots,
            truncated,
        })
    }

    /// The task's snapshots in announcement order with the invocations
    /// answered from each, and whether more snapshots than the limit exist.
    fn task_model_calls(&self, task: TaskId) -> Result<(ModelCalls, bool)> {
        let filter = EventFilter {
            task: Some(task),
            types: vec![
                "snapshot.created".into(),
                "invocation.completed".into(),
                "invocation.failed".into(),
            ],
            ..Default::default()
        };
        let mut calls = ModelCalls::default();
        let mut after = 0;
        loop {
            let page = self.store.events(after, PAGE, &filter)?;
            for event in &page {
                debug_assert!(event.seq > after, "events arrive in sequence order");
                after = event.seq;
                if !calls.record(&event.kind) {
                    return Ok((calls, true));
                }
            }
            if page.len() < PAGE {
                return Ok((calls, false));
            }
        }
    }

    /// The stored memory lists for `ids`, in order. An announced snapshot
    /// that is not stored is reported, never silently dropped.
    fn stored_memories(&self, ids: &[SnapshotId]) -> Result<Vec<SnapshotMemories>> {
        let found = self.store.snapshot_memories(ids)?;
        if found.len() != ids.len()
            && let Some(missing) = ids
                .iter()
                .find(|id| !found.iter().any(|row| row.snapshot == **id))
        {
            return Err(RuntimeError::NotFound(format!("snapshot {missing}")));
        }
        debug_assert_eq!(found.len(), ids.len(), "the callers pass distinct ids");
        Ok(found)
    }
}

/// A task's model calls folded from its events: each snapshot once, with
/// the invocations that name it.
#[derive(Default)]
struct ModelCalls {
    calls: Vec<(SnapshotId, Vec<InvocationId>)>,
    index: HashMap<SnapshotId, usize>,
}

impl ModelCalls {
    /// Fold one event in. `false` when a new snapshot would pass the limit.
    fn record(&mut self, kind: &EventKind) -> bool {
        match kind {
            // A resent native turn re-announces the same snapshot.
            EventKind::SnapshotCreated { snapshot, .. } if !self.index.contains_key(snapshot) => {
                if self.calls.len() == MEMORY_CONTEXT_LIMIT {
                    return false;
                }
                self.index.insert(*snapshot, self.calls.len());
                self.calls.push((*snapshot, Vec::new()));
            }
            EventKind::InvocationCompleted {
                invocation,
                snapshot,
                ..
            }
            | EventKind::InvocationFailed {
                invocation,
                snapshot,
                ..
            } => {
                if let Some(&at) = self.index.get(snapshot)
                    && !self.calls[at].1.contains(invocation)
                {
                    self.calls[at].1.push(*invocation);
                }
            }
            _ => {}
        }
        debug_assert_eq!(self.index.len(), self.calls.len());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::model::ModelRef;

    fn created(snapshot: SnapshotId) -> EventKind {
        EventKind::SnapshotCreated {
            snapshot,
            turn: 1,
            model: ModelRef::new("mock", "m"),
            token_estimate: 0,
            layers: vec![],
        }
    }

    fn completed(invocation: InvocationId, snapshot: SnapshotId) -> EventKind {
        EventKind::InvocationCompleted {
            invocation,
            snapshot,
            model: ModelRef::new("mock", "m"),
            usage: Default::default(),
            stop_reason: None,
            duration_ms: 0,
        }
    }

    #[test]
    fn calls_fold_once_per_snapshot_and_stop_at_the_limit() {
        let mut calls = ModelCalls::default();
        let first = SnapshotId::new();
        let (a, b) = (InvocationId::new(), InvocationId::new());
        for kind in [
            created(first),
            completed(a, first),
            created(first),
            completed(b, first),
            completed(b, first),
            completed(InvocationId::new(), SnapshotId::new()),
        ] {
            assert!(calls.record(&kind));
        }
        assert_eq!(calls.calls, vec![(first, vec![a, b])]);
        for _ in 1..MEMORY_CONTEXT_LIMIT {
            assert!(calls.record(&created(SnapshotId::new())));
        }
        assert_eq!(calls.calls.len(), MEMORY_CONTEXT_LIMIT);
        assert!(
            calls.record(&created(first)),
            "a known snapshot never overflows"
        );
        assert!(!calls.record(&created(SnapshotId::new())));
        assert_eq!(calls.calls.len(), MEMORY_CONTEXT_LIMIT);
    }
}
