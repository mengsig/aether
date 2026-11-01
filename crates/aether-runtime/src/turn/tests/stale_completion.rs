use super::*;

async fn budget_terminated_turn() -> (Arc<Runtime>, TurnCtx, AgentId, tempfile::TempDir) {
    let (runtime, dir) = crate::test_runtime(|_| {}).await;
    runtime.shutdown().await;
    let (creator, _) = runtime
        .create_agent(
            CreateAgentSpec {
                start: false,
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("creator");
    let (agent, _) = runtime
        .create_agent(
            CreateAgentSpec {
                objective: Some("finish the work".into()),
                start: true,
                config: Some(aether_core::agent::AgentConfig {
                    cost_budget: Some(1.0),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::agent(creator.id),
        )
        .expect("worker");
    let (ctx, _) = begin_turn(&runtime, agent.id, &CancellationToken::new())
        .expect("begin turn")
        .expect("active turn");
    assert_eq!(
        ctx.task.as_ref().unwrap().status.kind(),
        TaskStatusKind::Active
    );
    let mut spent = runtime.store.agent(agent.id).unwrap().unwrap();
    spent.stats.usage.cost_usd = 2.0;
    runtime
        .apply(Commit::new().mutate(Mutation::UpsertAgent(spent)))
        .expect("record spend");
    crate::scheduler::tick(&runtime).expect("budget sweep");
    assert_budget_failure(&runtime, &ctx, creator.id);
    (runtime, ctx, creator.id, dir)
}

fn assert_budget_failure(runtime: &Runtime, ctx: &TurnCtx, creator: AgentId) {
    assert_eq!(
        runtime
            .store
            .agent(ctx.agent.id)
            .unwrap()
            .unwrap()
            .status
            .kind(),
        AgentStatusKind::Failed
    );
    let task_id = ctx.task.as_ref().unwrap().id;
    let task = runtime.store.task(task_id).unwrap().unwrap();
    let TaskStatus::Failed { error, .. } = task.status else {
        panic!("budget failure overwritten: {:?}", task.status);
    };
    assert!(error.contains("budget"), "{error}");
    let terminal_events = runtime
        .store
        .events(0, 1000, &EventFilter::for_agent(ctx.agent.id))
        .unwrap()
        .into_iter()
        .filter(|event| {
            matches!(
                event.kind,
                EventKind::TaskCompleted { .. }
                    | EventKind::TaskFailed { .. }
                    | EventKind::TaskCancelled { .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal_events.len(), 1);
    assert!(matches!(
        terminal_events[0].kind,
        EventKind::TaskFailed { .. }
    ));
    let notifications = runtime
        .store
        .inbox(creator, 0)
        .unwrap()
        .into_iter()
        .filter(|item| {
            matches!(item.kind,
                InboxKind::TaskCompleted { task, .. }
                    | InboxKind::TaskFailed { task, .. }
                    | InboxKind::TaskCancelled { task, .. } if task == task_id
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(notifications.len(), 1);
    assert!(matches!(
        notifications[0].kind,
        InboxKind::TaskFailed { .. }
    ));
}

#[tokio::test]
async fn prose_completion_preserves_budget_termination() {
    let (runtime, ctx, creator, _dir) = budget_terminated_turn().await;
    let response = ModelResponse {
        content: vec![ContentBlock::text("finished after the budget expired")],
        ..Default::default()
    };
    let next = prose_only_next(
        &runtime,
        &ctx,
        &response,
        false,
        CompletionSource::NativeTurn(crate::turn::AnswerJobs::Idle),
    )
    .expect("prose completion");
    finish(&runtime, &ctx, next).expect("finish turn");
    assert_budget_failure(&runtime, &ctx, creator);
}

#[tokio::test]
async fn complete_task_preserves_budget_termination() {
    let (runtime, mut ctx, creator, _dir) = budget_terminated_turn().await;
    let outcome = apply_action(
        &runtime,
        &mut ctx,
        HarnessAction::CompleteTask {
            summary: "late completion".into(),
            artifacts: vec![],
            structured: None,
        },
        &mut ToolEffects::default(),
    )
    .await
    .expect("complete task action");
    assert_budget_failure(&runtime, &ctx, creator);
    assert!(outcome.is_error);
    assert!(outcome.content.contains("already failed"));
}

#[tokio::test]
async fn fail_task_preserves_budget_termination() {
    let (runtime, mut ctx, creator, _dir) = budget_terminated_turn().await;
    let outcome = apply_action(
        &runtime,
        &mut ctx,
        HarnessAction::FailTask {
            error: "late failure".into(),
            structured: None,
        },
        &mut ToolEffects::default(),
    )
    .await
    .expect("fail task action");
    assert_budget_failure(&runtime, &ctx, creator);
    assert!(outcome.is_error);
    assert!(outcome.content.contains("already failed"));
}
