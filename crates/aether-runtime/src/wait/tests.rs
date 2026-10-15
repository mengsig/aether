use super::*;
use aether_core::id::{InboxItemId, RunId};
use aether_core::inbox::InboxItem;
use aether_core::task::{Task, TaskResult, TaskStatus};
use aether_store::{Commit, Mutation};
use tokio_util::sync::CancellationToken;

fn flight(runtime: &crate::Runtime, owner: AgentId, task: TaskId) {
    runtime.sched.lock().running.insert(
        owner,
        crate::RunningTurn {
            task: Some(task),
            cancel: CancellationToken::new(),
            cancel_request: None,
            stop_reason: None,
            interrupt: CancellationToken::new(),
            addressed: false,
            provider: "claude-code".into(),
            engine: true,
            on_provider: true,
        },
    );
}

#[tokio::test]
async fn a_stored_terminal_task_waits_for_its_producing_turn_not_later_turns() {
    let (runtime, _dir) = crate::test_runtime(|_| {}).await;
    let owner = AgentId::new();
    let mut task = Task::new("done", Actor::user(), RunId::new(), now());
    task.transition(TaskStatus::Assigned, now()).unwrap();
    task.transition(TaskStatus::Active, now()).unwrap();
    task.transition(
        TaskStatus::Completed {
            result: TaskResult::summary("done"),
        },
        now(),
    )
    .unwrap();
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertTask(task.clone())))
        .unwrap();
    let oracle = RuntimeOracle {
        runtime: &runtime,
        agent: AgentId::new(),
        inbox_cursor: 0,
    };
    assert!(oracle.task_is_terminal(task.id));
    flight(&runtime, owner, task.id);
    let stored = StoreOracle {
        store: runtime.store.as_ref(),
        agent: oracle.agent,
        inbox_cursor: 0,
    };
    assert!(
        stored.task_is_terminal(task.id),
        "the public durable result remains visible"
    );
    assert!(
        !oracle.task_is_terminal(task.id),
        "the scheduler must await quiescence"
    );
    runtime.sched.lock().running.get_mut(&owner).unwrap().task = Some(TaskId::new());
    assert!(
        oracle.task_is_terminal(task.id),
        "later work cannot hold an earlier result hostage"
    );
    runtime.sched.lock().running.remove(&owner);
    runtime.shutdown().await;
}

#[tokio::test]
async fn settlement_notifications_wait_but_other_inbox_work_remains_reachable() {
    let (runtime, _dir) = crate::test_runtime(|_| {}).await;
    let owner = AgentId::new();
    let task = TaskId::new();
    flight(&runtime, owner, task);
    let kinds = [
        InboxKind::TaskCompleted {
            task,
            result: TaskResult::summary("done"),
        },
        InboxKind::TaskFailed {
            task,
            error: "failed".into(),
            structured: None,
        },
        InboxKind::TaskCancelled { task, reason: None },
        InboxKind::ChildFailed {
            agent: owner,
            error: "failed".into(),
        },
    ];
    for kind in kinds {
        let agent = AgentId::new();
        runtime
            .apply(Commit::new().mutate(Mutation::DeliverInbox(InboxItem {
                id: InboxItemId::new(),
                agent_id: agent,
                seq: 1,
                kind: kind.clone(),
                delivered_at: now(),
            })))
            .unwrap();
        let oracle = RuntimeOracle {
            runtime: &runtime,
            agent,
            inbox_cursor: 0,
        };
        assert!(!oracle.has_unconsumed_inbox(), "{kind:?}");
        runtime
            .apply(Commit::new().mutate(Mutation::DeliverInbox(InboxItem {
                id: InboxItemId::new(),
                agent_id: agent,
                seq: 2,
                kind: InboxKind::Wake {
                    reason: "independent work".into(),
                },
                delivered_at: now(),
            })))
            .unwrap();
        assert!(oracle.has_unconsumed_inbox());
    }
    runtime.sched.lock().running.remove(&owner);
    assert!(!settlement_in_flight(
        &runtime,
        &InboxKind::TaskCancelled { task, reason: None }
    ));
    runtime.shutdown().await;
}
