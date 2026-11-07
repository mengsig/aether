//! Lesson text streams as it is written, so the seal must hold before a
//! step's tool calls are known: nothing live shows an unrevealed answer.

use aether_noesis::teach::WITHHELD;

use super::*;

/// The model gives away its own key in streamed text, then poses it. The
/// learner saw the text, so the question is refused and never opens.
#[tokio::test(flavor = "multi_thread")]
async fn a_key_the_streamed_text_showed_is_never_posed() {
    let home = Home::new();
    let cli = Arc::new(FauxCli::default());
    let ask = json!({"kind": "recall", "prompt": "Name the code", "answer_key": "SECRET-ANSWER"});
    for _ in 0..2 {
        cli.queue(
            "prober",
            json!({"text": "It is SECRET-ANSWER.", "tool_calls": [tool_call("noesis_ask", ask.clone())]}),
        );
    }
    cli.queue(
        "prober",
        json!({"text": "Tell me what you know.", "tool_calls": []}),
    );
    let harness = engine_harness(&home, cli.clone()).await;
    let mut bus = harness.daemon.runtime.subscribe();
    let created = harness.create(json!({})).await;
    harness.settled().await;

    let streamed = live_text(&mut bus, agent_of(&created));
    assert!(
        streamed.contains("SECRET-ANSWER"),
        "the text streamed: {streamed}"
    );
    let session = harness.session(&created.id).await.expect("session");
    assert!(
        session.open_question.is_none(),
        "the shown key opened no question"
    );
    assert!(session.questions.is_empty(), "{:?}", session.questions);
    harness.close().await;
}

/// A step that runs while a problem's solution is unrevealed streams no
/// text, and its transcript keeps the text only as sealed.
#[tokio::test(flavor = "multi_thread")]
async fn a_step_while_a_solution_is_live_streams_nothing_and_is_kept_sealed() {
    let home = Home::new();
    let cli = Arc::new(FauxCli::default());
    let problem = json!({"statement": "What is 2 + 3?", "mode": "recall", "targets": [],
        "solution": SOLUTION_SECRET});
    cli.queue(
        "practitioner",
        json!({"text": "Try this one.", "tool_calls": [tool_call("noesis_problem", problem)]}),
    );
    let leak = format!("Psst: {SOLUTION_SECRET}");
    cli.queue("practitioner", json!({"text": leak, "tool_calls": []}));
    let harness = engine_harness(&home, cli.clone()).await;
    let mut bus = harness.daemon.runtime.subscribe();
    let created = harness.create(json!({"mode": "practice"})).await;
    harness.settled().await;
    let practitioner = agent_of(&created);

    let streamed = live_text(&mut bus, practitioner);
    assert!(streamed.contains("Try this one."), "{streamed}");
    assert!(
        !streamed.contains("Psst"),
        "a step under a live seal streamed: {streamed}"
    );
    let transcript = serde_json::to_string(&harness.transcript(practitioner)).expect("json");
    assert!(!transcript.contains(SOLUTION_SECRET), "{transcript}");
    assert!(transcript.contains(WITHHELD), "{transcript}");
    harness.close().await;
}
