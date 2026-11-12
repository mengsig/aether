use super::*;

/// A started worker's first turn on its task, delegated by a creator agent.
async fn worker_turn() -> (Arc<Runtime>, TurnCtx, AgentId, tempfile::TempDir) {
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
                objective: Some("review the change".into()),
                start: true,
                ..Default::default()
            },
            Actor::agent(creator.id),
        )
        .expect("worker");
    let (ctx, _) = begin_turn(&runtime, agent.id, &CancellationToken::new())
        .expect("begin turn")
        .expect("active turn");
    (runtime, ctx, creator.id, dir)
}

fn answer_in_prose(runtime: &Runtime, ctx: &TurnCtx, text: &str) -> TaskResult {
    let response = ModelResponse {
        content: vec![ContentBlock::text(text)],
        ..Default::default()
    };
    let next = prose_only_next(
        runtime,
        ctx,
        &response,
        true,
        CompletionSource::NativeTurn(crate::turn::AnswerJobs::Idle),
    )
    .expect("prose completion");
    finish(runtime, ctx, next).expect("finish turn");
    let task = runtime
        .store
        .task(ctx.task.as_ref().unwrap().id)
        .unwrap()
        .unwrap();
    let TaskStatus::Completed { result } = task.status else {
        panic!(
            "the prose answer did not complete the task: {:?}",
            task.status
        );
    };
    result
}

#[tokio::test]
async fn a_verdict_written_as_the_answer_reaches_the_creator_as_structured() {
    let (runtime, ctx, creator, _dir) = worker_turn().await;
    let verdict = serde_json::json!({"verdict": "pass", "findings": []});
    let text = serde_json::json!({"complete_task": verdict}).to_string();
    let result = answer_in_prose(&runtime, &ctx, &text);
    assert_eq!(result.structured.as_ref(), Some(&verdict));
    assert_eq!(result.summary, text);
    let delivered = runtime
        .store
        .inbox(creator, 0)
        .unwrap()
        .into_iter()
        .find_map(|item| match item.kind {
            InboxKind::TaskCompleted { result, .. } => Some(result),
            _ => None,
        })
        .expect("the creator hears of the completion");
    assert_eq!(delivered, result, "the creator gets the same result");
}

#[tokio::test]
async fn a_plain_prose_answer_still_completes_with_no_structured_result() {
    let (runtime, ctx, _creator, _dir) = worker_turn().await;
    let result = answer_in_prose(&runtime, &ctx, "Reviewed it; no findings.");
    assert_eq!(result, TaskResult::summary("Reviewed it; no findings."));
}
