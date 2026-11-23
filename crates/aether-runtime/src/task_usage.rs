use std::collections::{BTreeMap, BTreeSet};

use aether_core::event::EventKind;
use aether_core::id::{AgentId, TaskId};
use aether_core::model::Usage;
use aether_core::task::Task;
use aether_store::{EventFilter, StoreError, TaskFilter};

use crate::{Result, Runtime, RuntimeError};

const PAGE_SIZE: usize = 512;

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
        let mut pending = vec![task.clone()];
        let mut visited = BTreeSet::new();
        let mut delegated = Some(Usage::default());
        while let Some(current) = pending.pop() {
            let id = current.id;
            if !visited.insert(id) {
                return Err(RuntimeError::Internal(format!(
                    "cycle in task ancestry at {id}"
                )));
            }
            if id != task.id && scope.is_some_and(|ids| !crate::inspection::visible(&current, ids))
            {
                continue;
            }
            let usage = match self.task_own_usage(id) {
                Ok(usage) => Some(usage),
                Err(RuntimeError::Store(StoreError::HistoryExpired { .. })) => None,
                Err(error) => return Err(error),
            };
            if id == task.id {
                task.own_usage = usage;
            } else {
                delegated = match (delegated, usage) {
                    (Some(mut total), Some(usage)) => {
                        total += usage;
                        Some(total)
                    }
                    _ => None,
                };
            }
            let children = self.store.tasks(&TaskFilter {
                parent_task: Some(id),
                ..TaskFilter::default()
            })?;
            debug_assert!(children.iter().all(|child| child.parent_task == Some(id)));
            pending.extend(children);
        }
        debug_assert!(visited.contains(&task.id));
        task.delegated_usage = delegated;
        Ok(task)
    }

    fn task_own_usage(&self, id: TaskId) -> Result<Usage> {
        let filter = EventFilter {
            task: Some(id),
            types: vec![
                "agent.started".into(),
                "agent.usage_updated".into(),
                "invocation.completed".into(),
            ],
            ..EventFilter::default()
        };
        let mut turns = BTreeMap::<AgentId, (Usage, bool)>::new();
        let mut total = Usage::default();
        let mut after = 0;
        loop {
            let events = self.store.events(after, PAGE_SIZE, &filter)?;
            debug_assert!(events.len() <= PAGE_SIZE);
            if events.is_empty() {
                break;
            }
            for event in &events {
                debug_assert!(event.seq > after);
                after = event.seq;
                let Some(agent) = event.agent_id else {
                    continue;
                };
                match &event.kind {
                    EventKind::AgentStarted { .. } => {
                        if let Some((previous, _)) = turns.insert(agent, (Usage::default(), false))
                        {
                            total += previous;
                        }
                    }
                    EventKind::AgentUsageUpdated { turn_usage, .. } => {
                        if let Some((turn, has_snapshot)) = turns.get_mut(&agent) {
                            *turn = *turn_usage;
                            *has_snapshot = true;
                        }
                    }
                    EventKind::InvocationCompleted { usage, .. } => {
                        if let Some((turn, has_snapshot)) = turns.get_mut(&agent)
                            && (!*has_snapshot
                                || usage.total_tokens() > 0
                                || usage.has_cost_figure())
                        {
                            *turn += *usage;
                        }
                    }
                    _ => {}
                }
            }
            if events.len() < PAGE_SIZE {
                break;
            }
        }
        for (usage, _) in turns.into_values() {
            total += usage;
        }
        Ok(total)
    }
}
