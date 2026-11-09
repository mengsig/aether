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

/// One taught step as the model produces it: a reply with prose beside its
/// card, the explanation it is then asked for, and the card posed alone.
fn queue_step(cli: &FauxCli, mixed: &str, lesson: &str, card: &Value) {
    cli.queue(
        "teacher",
        json!({"text": mixed, "tool_calls": [tool_call(ASK, card.clone())]}),
    );
    cli.queue("teacher", json!({"text": lesson}));
    cli.queue(
        "teacher",
        json!({"tool_calls": [tool_call(ASK, card.clone())]}),
    );
}

fn taught_steps() -> [(
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
); 3] {
    [
        (
            "Four corners make a turn. Which shape has three sides?",
            "Four corners make a turn.",
            "a",
            "How many degrees are in a full turn?",
            "360 degrees",
            "90 degrees",
        ),
        (
            "A half turn is two quarter turns. Name a shape with three sides",
            "A half turn is two quarter turns.",
            "b",
            "How many sides does a hexagon have?",
            "6 sides",
            "8 sides",
        ),
        (
            "Triangles have three corners. Tell me the sum of their angles",
            "Triangles have three corners.",
            "c",
            "What do the angles of a triangle add up to?",
            "180 degrees",
            "270 degrees",
        ),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn go_and_each_passed_check_teach_then_pose_with_no_prose_question() {
    let home = Home::new();
    let cli = Arc::new(FauxCli::default());
    let plan = json!({"nodes": [{"id": "a", "title": "Turns"}, {"id": "b", "title": "Sides"}, {"id": "c", "title": "Sums"}],
        "edges": [{"from": "a", "to": "b"}, {"from": "b", "to": "c"}]});
    let harness = engine_harness(&home, cli.clone()).await;
    let id = start_teaching(&harness, &cli, plan).await;
    let steps = taught_steps();
    for (mixed, lesson, node, prompt, right, wrong) in &steps {
        queue_step(&cli, mixed, lesson, &check(prompt, right, wrong, node));
    }
    for (index, (_, _, _, prompt, right, _)) in steps.iter().enumerate() {
        if index == 0 {
            harness
                .send_json(json!({"cmd": "noesis_send", "session": id, "text": "Go", "idempotency_key": "go"}))
                .await
                .expect("go");
        }
        let posed = harness.posed(&id).await;
        harness.settled().await;
        assert_eq!(
            posed.open_question.as_ref().expect("a card").prompt,
            *prompt
        );
        harness.answer(&posed, right).await.expect("answer");
        harness.settled().await;
    }
    let agent = agent_of(&harness.session(&id).await.expect("session"));
    let texts = assistant_texts(&harness, agent);
    for (_, lesson, ..) in steps {
        assert!(
            texts.iter().any(|text| text == lesson),
            "{lesson} missing from {texts:?}"
        );
    }
    for question in ["Which shape", "Name a shape", "Tell me the sum"] {
        assert!(
            texts.iter().all(|text| !text.contains(question)),
            "{question} in {texts:?}"
        );
    }
    harness.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_pose_and_its_replacement_leave_no_prose_question() {
    let home = Home::new();
    let cli = Arc::new(FauxCli::default());
    let plan =
        json!({"nodes": [{"id": "turns", "title": "Right angle = 360° ÷ 4 = 90°"}], "edges": []});
    let harness = engine_harness(&home, cli.clone()).await;
    let id = start_teaching(&harness, &cli, plan).await;
    let first = check(
        "How many right angles fit around one point?",
        "4",
        "2",
        "turns",
    );
    let fresh = check(
        "How many degrees are in half a turn?",
        "180°",
        "270°",
        "turns",
    );
    cli.queue("teacher", json!({"text": "Four corners make a turn. Pick the count of right angles around a point", "tool_calls": [tool_call(ASK, first.clone())]}));
    cli.queue("teacher", json!({"text": "Four corners make a turn."}));
    cli.queue("teacher", json!({"tool_calls": [tool_call(ASK, first)]}));
    cli.queue("teacher", json!({"text": "Different idea. How many sides has a triangle", "tool_calls": [tool_call(ASK, fresh)]}));
    harness
        .send_json(
            json!({"cmd": "noesis_send", "session": id, "text": "Go", "idempotency_key": "go"}),
        )
        .await
        .expect("go");
    let posed = harness.posed(&id).await;
    harness.settled().await;
    let texts = assistant_texts(&harness, agent_of(&posed)).join("\n");
    assert!(texts.contains("Four corners make a turn."), "{texts}");
    for question in ["Pick the count", "How many sides", "Different idea"] {
        assert!(!texts.contains(question), "{question} recorded in {texts}");
    }
    let shown = posed.open_question.expect("the replacement is open");
    assert_eq!(shown.prompt, "How many degrees are in half a turn?");
    harness.close().await;
}
