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

/// No event and no transcript entry of `agent` holds `secret`.
fn assert_never_kept(harness: &Harness, agent: AgentId, secret: &str) {
    let events = serde_json::to_string(&harness.events()).expect("json");
    assert!(!events.contains(secret), "an event holds it: {events}");
    let transcript = serde_json::to_string(&harness.transcript(agent)).expect("json");
    assert!(!transcript.contains(secret), "{transcript}");
}

/// An attempt streams its key, then breaks off malformed and is retried.
/// The learner saw the key, so the retry's pose of it is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_key_a_malformed_attempt_streamed_is_never_posed() {
    let home = Home::new();
    let cli = Arc::new(FauxCli::default());
    let broken = r#"{"text": "It is SECRET-ANSWER.", "tool_calls": [{"name": "noesis_ask""#;
    cli.queue("prober", Value::String(broken.to_string()));
    let ask = json!({"kind": "recall", "prompt": "Name the code", "answer_key": "SECRET-ANSWER"});
    cli.queue(
        "prober",
        json!({"text": "Your turn:", "tool_calls": [tool_call("noesis_ask", ask)]}),
    );
    cli.queue("prober", json!({"text": "Tell me more.", "tool_calls": []}));
    let harness = engine_harness(&home, cli.clone()).await;
    let mut bus = harness.daemon.runtime.subscribe();
    let created = harness.create(json!({})).await;
    harness.settled().await;

    let streamed = live_text(&mut bus, agent_of(&created));
    assert!(
        streamed.contains("SECRET-ANSWER"),
        "the attempt streamed: {streamed}"
    );
    let session = harness.session(&created.id).await.expect("session");
    assert!(
        session.open_question.is_none(),
        "the streamed key opened no question"
    );
    assert!(session.questions.is_empty(), "{:?}", session.questions);
    harness.close().await;
}

/// While a problem's solution is live, a call that names it, a search or a
/// refused second problem, is kept with that input sealed.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_naming_a_live_solution_is_kept_sealed() {
    let home = Home::new();
    let cli = Arc::new(FauxCli::default());
    let problem = json!({"statement": "What is 2 + 3?", "mode": "recall", "targets": [],
        "solution": SOLUTION_SECRET});
    cli.queue(
        "practitioner",
        json!({"text": "Try this one.", "tool_calls": [tool_call("noesis_problem", problem)]}),
    );
    let again = json!({"statement": format!("Again: {SOLUTION_SECRET}"), "mode": "recall",
        "targets": [], "solution": "seven"});
    let search = json!({"query": SOLUTION_SECRET});
    cli.queue(
        "practitioner",
        json!({"text": "Let me look.", "tool_calls": [
            tool_call("noesis_graph_search", search), tool_call("noesis_problem", again)]}),
    );
    cli.queue(
        "practitioner",
        json!({"text": "Over to you.", "tool_calls": []}),
    );
    let harness = engine_harness(&home, cli.clone()).await;
    let created = harness.create(json!({"mode": "practice"})).await;
    harness.settled().await;

    let practitioner = agent_of(&created);
    assert_never_kept(&harness, practitioner, SOLUTION_SECRET);
    let transcript = serde_json::to_string(&harness.transcript(practitioner)).expect("json");
    assert!(transcript.contains("Let me look."), "{transcript}");
    assert!(transcript.contains("<sealed>"), "{transcript}");
    harness.close().await;
}

/// A reply that poses a question and names its key in another call keeps
/// that call sealed, and the question still opens.
#[tokio::test(flavor = "multi_thread")]
async fn a_key_named_by_another_call_of_its_posing_reply_is_kept_sealed() {
    let home = Home::new();
    let cli = Arc::new(FauxCli::default());
    let ask = json!({"kind": "recall", "prompt": "Name the code", "answer_key": "SECRET-ANSWER"});
    cli.queue(
        "prober",
        json!({"text": "One question:", "tool_calls": [
            tool_call("noesis_graph_search", json!({"query": "SECRET-ANSWER"})),
            tool_call("noesis_ask", ask)]}),
    );
    let harness = engine_harness(&home, cli.clone()).await;
    let created = harness.create(json!({})).await;
    harness.settled().await;

    let session = harness.session(&created.id).await.expect("session");
    assert!(session.open_question.is_some(), "the question opened");
    assert_never_kept(&harness, agent_of(&created), "SECRET-ANSWER");
    harness.close().await;
}
