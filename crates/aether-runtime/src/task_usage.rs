use std::collections::{BTreeMap, BTreeSet};

use aether_core::event::{Event, EventKind};
use aether_core::id::{AgentId, TaskId};
use aether_core::model::{CostGap, Usage};
use aether_core::task::Task;
use aether_store::{EventFilter, StoreError, TaskFilter};

use crate::{Result, Runtime, RuntimeError};

const PAGE_SIZE: usize = 512;

#[derive(Debug, Default)]
struct TurnUsage {
    usage: Usage,
    has_snapshot: bool,
}

impl Runtime {
    /// Read a task with usage derived from its retained task-tagged events.
    pub fn get_task_with_usage(&self, id: TaskId) -> Result<Option<Task>> {
        self.store
            .task(id)?
            .map(|task| self.project_task_usage(task, None))
            .transpose()
    }

    pub(crate) fn project_task_usage(
        &self,
        mut task: Task,
        scope: Option<&BTreeSet<AgentId>>,
    ) -> Result<Task> {
        debug_assert!(scope.is_none_or(|ids| crate::inspection::visible(&task, ids)));
        let through = self.store.last_seq()?;
        task.own_usage = self.task_usage_or_expired(task.id, through)?;
        task.delegated_usage = self.delegated_task_usage(task.id, scope, through)?;
        Ok(task)
    }

    fn delegated_task_usage(
        &self,
        root: TaskId,
        scope: Option<&BTreeSet<AgentId>>,
        through: u64,
    ) -> Result<Option<Usage>> {
        let mut pending = vec![root];
        let mut visited = BTreeSet::from([root]);
        let mut delegated = Usage::default();
        while let Some(parent) = pending.pop() {
            debug_assert!(visited.contains(&parent));
            for child in self.store.tasks(&TaskFilter {
                parent_task: Some(parent),
                ..TaskFilter::default()
            })? {
                debug_assert_eq!(child.parent_task, Some(parent));
                if scope.is_some_and(|ids| !crate::inspection::visible(&child, ids)) {
                    continue;
                }
                if !visited.insert(child.id) {
                    return Err(RuntimeError::Internal(format!(
                        "cycle in task ancestry at {}",
                        child.id
                    )));
                }
                let Some(usage) = self.task_usage_or_expired(child.id, through)? else {
                    return Ok(None);
                };
                delegated += usage;
                pending.push(child.id);
            }
        }
        Ok(Some(delegated))
    }

    fn task_usage_or_expired(&self, id: TaskId, through: u64) -> Result<Option<Usage>> {
        match self.task_own_usage(id, through) {
            Ok(usage) => Ok(Some(usage)),
            Err(RuntimeError::Store(StoreError::HistoryExpired { .. })) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn task_own_usage(&self, id: TaskId, through: u64) -> Result<Usage> {
        let filter = EventFilter {
            task: Some(id),
            types: vec![
                "agent.started".into(),
                "agent.usage_updated".into(),
                "invocation.completed".into(),
                "invocation.failed".into(),
            ],
            ..EventFilter::default()
        };
        let mut turns = BTreeMap::<AgentId, TurnUsage>::new();
        let mut total = Usage::default();
        let mut after = 0;
        'pages: while after < through {
            let events = self.store.events(after, PAGE_SIZE, &filter)?;
            debug_assert!(events.len() <= PAGE_SIZE);
            for event in &events {
                debug_assert_eq!(event.task_id, Some(id));
                debug_assert!(event.seq > after);
                if event.seq > through {
                    break 'pages;
                }
                after = event.seq;
                self.fold_task_event(event, &mut turns, &mut total)?;
            }
            if events.len() < PAGE_SIZE {
                break;
            }
        }
        for turn in turns.into_values() {
            total += turn.usage;
        }
        Ok(total)
    }

    fn fold_task_event(
        &self,
        event: &Event,
        turns: &mut BTreeMap<AgentId, TurnUsage>,
        total: &mut Usage,
    ) -> Result<()> {
        let Some(agent) = event.agent_id else {
            return Ok(());
        };
        debug_assert!(event.task_id.is_some());
        if matches!(&event.kind, EventKind::AgentStarted { .. }) {
            if let Some(previous) = turns.insert(agent, TurnUsage::default()) {
                *total += previous.usage;
            }
            return Ok(());
        }
        let Some(turn) = turns.get_mut(&agent) else {
            return Ok(());
        };
        match &event.kind {
            EventKind::AgentUsageUpdated { turn_usage, .. } => {
                turn.usage = *turn_usage;
                turn.has_snapshot = true;
            }
            EventKind::InvocationCompleted { usage, .. } => {
                // Native message placeholders carry no counters; the turn snapshot covers them.
                let placeholder = usage.total_tokens() == 0
                    && !usage.has_cost_figure()
                    && usage.cost_gap == Some(CostGap::Unreported);
                if !turn.has_snapshot || !placeholder {
                    turn.usage += *usage;
                }
            }
            EventKind::InvocationFailed {
                invocation,
                snapshot,
                ..
            } => {
                if let Some(record) = self.store.invocation(*invocation)? {
                    debug_assert_eq!(record.id, *invocation);
                    if record.agent_id != agent || record.snapshot_id != *snapshot {
                        return Err(RuntimeError::Internal(format!(
                            "failed invocation {invocation} does not match its task event"
                        )));
                    }
                    turn.usage += record.usage;
                } else {
                    turn.usage.mark_cost_gap(CostGap::Unreported);
                }
            }
            _ => {}
        }
        Ok(())
    }
}
