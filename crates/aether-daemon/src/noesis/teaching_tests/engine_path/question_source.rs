//! The learner answers the card, including after a refused pose and replacement.

use super::*;

/// Every assistant text block an agent's transcript holds, in order.
fn assistant_texts(harness: &Harness, agent: AgentId) -> Vec<String> {
    harness
        .transcript(agent)
        .iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn check(prompt: &str, right: &str, wrong: &str, node: &str) -> Value {
    json!({"kind": "choice", "prompt": prompt,
        "options": [{"value": "opt1", "label": right}, {"value": "opt2", "label": wrong}],
        "answer_key": [right], "explanation": format!("Because {right} is correct."),
        "plan_node": node})
}

async fn start_teaching(harness: &Harness, cli: &FauxCli, plan: Value) -> String {
    cli.queue(
        "prober",
        json!({"tool_calls": [tool_call(ASK, capital_question())]}),
    );
    cli.queue("prober", json!({"tool_calls": [tool_call("noesis_probe_done", json!({"strands": [{"strand": "angles", "floor": "corners", "ceiling": "turns", "start": "turns"}]}))]}));
    cli.queue(
        "teacher",
        json!({"text": "Here is the plan.", "tool_calls": [tool_call("noesis_plan", plan)]}),
    );
    let created = harness.create(json!({"goal": "Learn angles"})).await;
    let probe = harness.posed(&created.id).await;
    harness.answer(&probe, "Paris").await.expect("probe answer");
    harness.settled().await;
    created.id
}

#[tokio::test(flavor = "multi_thread")]
async fn only_prose_from_the_accepted_check_reaches_the_transcript() {
    let home = Home::new();
    let cli = Arc::new(FauxCli::default());
    let plan = json!({"nodes": [{"id": "turns", "title": "Turns"}], "edges": []});
    let harness = engine_harness(&home, cli.clone()).await;
    let id = start_teaching(&harness, &cli, plan.clone()).await;
    cli.queue(
        "teacher",
        json!({"text": "Name a triangle", "tool_calls": [tool_call("noesis_plan", plan)]}),
    );
    let mut card = check("How many degrees in a half turn?", "180°", "270°", "turns");
    card["lead_in"] = json!("A half turn is half a full rotation.");
    cli.queue(
        "teacher",
        json!({"text": "Which shape has three sides?", "tool_calls": [tool_call(ASK, card)]}),
    );
    harness
        .send_json(
            json!({"cmd": "noesis_send", "session": id, "text": "Go", "idempotency_key": "go"}),
        )
        .await
        .expect("go");
    let posed = harness.posed(&id).await;
    harness.settled().await;
    let texts = assistant_texts(&harness, agent_of(&posed)).join("\n");
    assert!(!texts.contains("Name a triangle"), "{texts}");
    assert!(!texts.contains("Which shape has three sides?"), "{texts}");
    assert!(
        texts.contains("A half turn is half a full rotation."),
        "{texts}"
    );
    assert_eq!(
        posed.open_question.as_ref().expect("card").prompt,
        "How many degrees in a half turn?"
    );
    harness.answer(&posed, "180°").await.expect("answer");
    harness.settled().await;
    let transcript = harness.transcript(agent_of(&posed));
    assert!(transcript.iter().any(|message| message.role == MessageRole::User
        && message.content.iter().any(|block| matches!(block, ContentBlock::ToolResult { is_error: false, .. }))
        && message.content.iter().any(|block| matches!(block, ContentBlock::Text { text } if text.starts_with("The learner answered:")))));
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn refused_check_prose_is_not_recorded_before_a_replacement() {
    let home = Home::new();
    let cli = Arc::new(FauxCli::default());
    let plan =
        json!({"nodes": [{"id": "turns", "title": "Right angle = 360° ÷ 4 = 90°"}], "edges": []});
    let harness = engine_harness(&home, cli.clone()).await;
    let id = start_teaching(&harness, &cli, plan).await;
    let mut first = check(
        "How many right angles fit around one point?",
        "4",
        "2",
        "turns",
    );
    let mut fresh = check(
        "How many degrees are in half a turn?",
        "180°",
        "270°",
        "turns",
    );
    first["lead_in"] = json!("The refused lesson must not appear.");
    cli.queue("teacher", json!({"text": "Pick the count of right angles around a point", "tool_calls": [tool_call(ASK, first)]}));
    fresh["lead_in"] = json!("A half turn is half a full rotation.");
    cli.queue(
        "teacher",
        json!({"text": "Name a shape with three sides", "tool_calls": [tool_call(ASK, fresh)]}),
    );
    harness
        .send_json(
            json!({"cmd": "noesis_send", "session": id, "text": "Go", "idempotency_key": "go"}),
        )
        .await
        .expect("go");
    let posed = harness.posed(&id).await;
    harness.settled().await;
    let texts = assistant_texts(&harness, agent_of(&posed)).join("\n");
    assert!(!texts.contains("Pick the count"), "{texts}");
    assert!(
        !texts.contains("The refused lesson must not appear."),
        "{texts}"
    );
    let transcript = format!("{:?}", harness.transcript(agent_of(&posed)));
    assert!(
        !transcript.contains("The refused lesson must not appear."),
        "{transcript}"
    );
    assert!(!transcript.contains("lead_in"), "{transcript}");
    assert!(!texts.contains("Name a shape with three sides"), "{texts}");
    assert!(
        texts.contains("A half turn is half a full rotation."),
        "{texts}"
    );
    assert_eq!(
        posed.open_question.expect("replacement").prompt,
        "How many degrees are in half a turn?"
    );
    harness.close().await;
}
