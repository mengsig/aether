use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn answered_recall_is_judged_before_the_next_question() {
    let home = Home::new();
    let cli = Arc::new(FauxCli::default());
    let recall = json!({"kind": "recall", "prompt": "Solve 2x = 8", "answer_key": "x = 4"});
    cli.queue(
        "prober",
        json!({"text": "", "tool_calls": [tool_call(ASK, recall)]}),
    );
    let harness = engine_harness(&home, cli.clone()).await;
    let created = harness.create(json!({"goal": "Learn algebra"})).await;
    let posed = harness.posed(&created.id).await;
    harness.settled().await;
    let question = posed.open_question.as_ref().expect("recall").id.to_string();
    assert_sealed(&harness, posed.agent, "x = 4");

    cli.queue(
        "prober",
        json!({"text": "Next", "tool_calls": [tool_call(ASK, json!({"kind": "recall", "prompt": "What is 4 + 4?", "answer_key": "eight"}))]}),
    );
    cli.queue(
        "prober",
        json!({"text": "", "tool_calls": [
            tool_call("noesis_judge_recall", json!({"question": question, "correct": true})),
            tool_call(ASK, capital_question())
        ]}),
    );
    harness
        .send_json(json!({"cmd": "noesis_answer", "session": created.id,
        "question": question, "answer": {"text": "x = 4", "thinking": "divide both sides by 2", "dont_know": false},
        "idempotency_key": "recall-answer"}))
        .await
        .expect("answer");
    harness.settled().await;
    let view = harness.session(&created.id).await.expect("session");
    assert_eq!(view.questions.len(), 2);
    assert_eq!(
        view.questions[0].result.as_ref().unwrap().correct,
        Some(true)
    );
    assert!(view.open_question.is_some());
    let launches = cli.launches();
    let retry = launches.last().expect("retry");
    assert!(
        retry.prompt.contains("noesis.judge_recall")
            && retry.prompt.contains("divide both sides by 2")
    );
    assert_eq!(view.agents[0].internal.len(), 1);
    let transcript = serde_json::to_string(&harness.transcript(posed.agent)).unwrap();
    assert!(!transcript.contains("System guidance"));
    let events = serde_json::to_string(&harness.events()).unwrap();
    assert!(!events.contains("System guidance"));
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_judgment_stops_with_a_visible_retry_and_can_recover() {
    let home = Home::new();
    let cli = Arc::new(FauxCli::default());
    cli.queue(
        "prober",
        json!({"text": "", "tool_calls": [tool_call(ASK,
        json!({"kind": "recall", "prompt": "Solve 2x = 8", "answer_key": "x = 4"}))]}),
    );
    let harness = engine_harness(&home, cli.clone()).await;
    let created = harness.create(json!({})).await;
    let posed = harness.posed(&created.id).await;
    harness.settled().await;
    let question = posed.open_question.as_ref().unwrap().id.to_string();
    for _ in 0..3 {
        cli.queue("prober", json!({"text": "Moving on"}));
    }
    harness
        .send_json(json!({"cmd": "noesis_answer", "session": created.id,
        "question": question, "answer": {"text": "x = 4", "dont_know": false}, "idempotency_key": "answer"}))
        .await
        .expect("answer");
    harness.settled().await;
    let view = harness.session(&created.id).await.unwrap();
    assert_eq!(view.questions[0].result.as_ref().unwrap().correct, None);
    assert!(view.open_question.is_none());
    let transcript = harness.transcript(posed.agent);
    assert!(transcript.iter().any(|message| message.role == MessageRole::Assistant &&
        message.content.iter().any(|block| matches!(block, ContentBlock::Text { text } if text.contains("Try again")))));
    assert!(
        !serde_json::to_string(&transcript)
            .unwrap()
            .contains("System guidance")
    );
    let launches = cli.launches().len();
    assert_eq!(launches, 4, "one pose and three judgment attempts");
    cli.queue(
        "prober",
        json!({"text": "", "tool_calls": [tool_call("noesis_judge_recall",
        json!({"question": question, "correct": true}))]}),
    );
    cli.queue("prober", json!({"text": "Well done"}));
    harness
        .send_json(json!({"cmd": "noesis_retry_turn", "session": created.id,
        "idempotency_key": "retry"}))
        .await
        .expect("retry");
    harness.settled().await;
    let view = harness.session(&created.id).await.unwrap();
    assert_eq!(
        view.questions[0].result.as_ref().unwrap().correct,
        Some(true)
    );
    assert_eq!(cli.launches().len(), launches + 2);
    harness.close().await;
}
