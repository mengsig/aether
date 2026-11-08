//! The one seal check on a scripted engine: a lesson reply is buffered
//! whole, and one that gives a sealed answer away is discarded unseen,
//! made once more with a reminder, then replaced by the neutral step.

use aether_noesis::teach::{NEUTRAL_REPLY, SEAL_REMINDER};
use tokio::sync::broadcast::error::TryRecvError;

use super::*;

const KEY: &str = "SECRET-ANSWER";

fn ask() -> Value {
    json!({"kind": "recall", "prompt": "Name the code", "answer_key": KEY})
}

/// No event and no transcript entry of `agent` holds `secret`.
fn assert_never_kept(harness: &Harness, agent: AgentId, secret: &str) {
    let events = serde_json::to_string(&harness.events()).expect("json");
    assert!(!events.contains(secret), "an event holds it: {events}");
    let transcript = serde_json::to_string(&harness.transcript(agent)).expect("json");
    assert!(!transcript.contains(secret), "{transcript}");
}

/// How many of the engine's launches were told the seal's reminder.
fn reminded(cli: &FauxCli) -> usize {
    let launches = cli.launches();
    launches
        .iter()
        .filter(|launch| launch.prompt.contains(SEAL_REMINDER))
        .count()
}

/// The model gives its own key away beside the question. Nothing of that
/// reply streams or is kept; the retry, told why, poses it cleanly.
#[tokio::test(flavor = "multi_thread")]
async fn a_reply_giving_its_own_key_away_is_discarded_and_made_again() {
    let home = Home::new();
    let cli = Arc::new(FauxCli::default());
    let leak = format!("Hint: it is {KEY}.");
    cli.queue(
        "prober",
        json!({"text": leak, "tool_calls": [tool_call("noesis_ask", ask())]}),
    );
    cli.queue(
        "prober",
        json!({"text": "One question:", "tool_calls": [tool_call("noesis_ask", ask())]}),
    );
    let harness = engine_harness(&home, cli.clone()).await;
    let mut bus = harness.daemon.runtime.subscribe();
    let created = harness.create(json!({})).await;
    harness.settled().await;

    let prober = agent_of(&created);
    assert_eq!(
        live_text(&mut bus, prober),
        "",
        "a lesson reply never streams"
    );
    let session = harness.session(&created.id).await.expect("session");
    let open = session.open_question.expect("the retried question opened");
    assert_eq!(open.prompt, "Name the code");
    assert_never_kept(&harness, prober, KEY);
    assert_never_kept(&harness, prober, "Hint:");
    assert_eq!(reminded(&cli), 1, "the retry was told why");
    harness.close().await;
}

/// A clean step whose pose carries the hidden answer: the engine streams
/// the raw reply, key included, yet no message any client can receive
/// (a delta of any agent, an event) and no transcript entry holds it.
#[tokio::test(flavor = "multi_thread")]
async fn a_pose_carrying_its_answer_streams_nothing_that_holds_it() {
    let home = Home::new();
    let cli = Arc::new(FauxCli::default());
    cli.queue(
        "prober",
        json!({"text": "One question:", "tool_calls": [tool_call("noesis_ask", ask())]}),
    );
    let harness = engine_harness(&home, cli.clone()).await;
    let mut bus = harness.daemon.runtime.subscribe();
    let created = harness.create(json!({})).await;
    harness.settled().await;

    let prober = agent_of(&created);
    let mut deltas = 0;
    loop {
        let message = match bus.try_recv() {
            Ok(message) => message,
            Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            Err(TryRecvError::Lagged(missed)) => panic!("{missed} messages went unread"),
        };
        let shown = format!("{message:?}");
        assert!(!shown.contains(KEY), "a client could read it: {shown}");
        if matches!(message, BusMessage::Delta { agent, .. } if agent == prober) {
            deltas += 1;
        }
    }
    assert_eq!(deltas, 0, "a lesson reply is buffered whole");
    let session = harness.session(&created.id).await.expect("session");
    assert_eq!(
        session.open_question.expect("posed").prompt,
        "Name the code"
    );
    assert_never_kept(&harness, prober, KEY);
    assert_eq!(reminded(&cli), 0, "a clean pose is kept at once");
    harness.close().await;
}

/// Two replies in a row that give the key away become the neutral step:
/// nothing of either is kept, no question opens and the learner is asked on.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_leaking_reply_becomes_the_neutral_step() {
    let home = Home::new();
    let cli = Arc::new(FauxCli::default());
    let search = tool_call("noesis_graph_search", json!({"query": KEY}));
    for _ in 0..2 {
        cli.queue(
            "prober",
            json!({"text": "One question:", "tool_calls": [search.clone(), tool_call("noesis_ask", ask())]}),
        );
    }
    let harness = engine_harness(&home, cli.clone()).await;
    let created = harness.create(json!({})).await;
    harness.settled().await;

    let prober = agent_of(&created);
    let session = harness.session(&created.id).await.expect("session");
    assert!(session.open_question.is_none() && session.questions.is_empty());
    assert_never_kept(&harness, prober, KEY);
    let transcript = serde_json::to_string(&harness.transcript(prober)).expect("json");
    assert!(transcript.contains(NEUTRAL_REPLY), "{transcript}");
    assert!(!transcript.contains("One question:"), "{transcript}");
    assert_eq!(cli.launches().len(), 2, "one retry, no more");
    harness.close().await;
}

/// While a problem's solution is live, a reply that names it, in its text
/// or a call's input, is discarded and the clean retry is kept.
#[tokio::test(flavor = "multi_thread")]
async fn a_reply_naming_a_live_solution_is_discarded() {
    let home = Home::new();
    let cli = Arc::new(FauxCli::default());
    let problem = json!({"statement": "What is 2 + 3?", "mode": "recall", "targets": [],
        "solution": SOLUTION_SECRET});
    cli.queue(
        "practitioner",
        json!({"text": "Try this one.", "tool_calls": [tool_call("noesis_problem", problem)]}),
    );
    let harness = engine_harness(&home, cli.clone()).await;
    let created = harness.create(json!({"mode": "practice"})).await;
    harness.settled().await;
    let practitioner = agent_of(&created);
    let search = json!({"query": SOLUTION_SECRET});
    cli.queue(
        "practitioner",
        json!({"text": "Let me look.", "tool_calls": [tool_call("noesis_graph_search", search)]}),
    );
    cli.queue(
        "practitioner",
        json!({"text": "Over to you.", "tool_calls": []}),
    );

    let sent = json!({"cmd": "noesis_send", "session": created.id, "text": "A hint?",
        "idempotency_key": "hint"});
    harness.send_json(sent).await.expect("sent");
    harness.settled().await;
    assert_never_kept(&harness, practitioner, SOLUTION_SECRET);
    let transcript = serde_json::to_string(&harness.transcript(practitioner)).expect("json");
    assert!(transcript.contains("Over to you."), "{transcript}");
    assert!(!transcript.contains("Let me look."), "{transcript}");
    assert_eq!(reminded(&cli), 1);
    harness.close().await;
}

/// A question whose own prompt names its key, whether posing would accept
/// its form or refuse it, is discarded before its prompt reaches the
/// transcript, and the clean retry poses.
#[tokio::test(flavor = "multi_thread")]
async fn a_pose_naming_its_own_key_is_discarded_before_it_is_kept() {
    for kind in ["recall", "essay"] {
        let home = Home::new();
        let cli = Arc::new(FauxCli::default());
        let spoiled = json!({"kind": kind, "prompt": format!("Is the code {KEY}?"),
            "answer_key": KEY});
        cli.queue(
            "prober",
            json!({"text": "One question:", "tool_calls": [tool_call("noesis_ask", spoiled)]}),
        );
        cli.queue(
            "prober",
            json!({"text": "One question:", "tool_calls": [tool_call("noesis_ask", ask())]}),
        );
        let harness = engine_harness(&home, cli.clone()).await;
        let created = harness.create(json!({})).await;
        harness.settled().await;

        let prober = agent_of(&created);
        let session = harness.session(&created.id).await.expect("session");
        let open = session.open_question.expect("the retried question opened");
        assert_eq!(open.prompt, "Name the code", "{kind}");
        assert_never_kept(&harness, prober, KEY);
        assert_never_kept(&harness, prober, "Is the code");
        assert_eq!(reminded(&cli), 1, "{kind}: the retry was told why");
        harness.close().await;
    }
}

/// A conclusion the handler refuses leaves the problem active, so a reply
/// that names the solution beside it is discarded; the retry concludes.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_conclusion_keeps_the_solution_sealed() {
    let home = Home::new();
    let cli = Arc::new(FauxCli::default());
    let problem = json!({"statement": "What is 2 + 3?", "mode": "recall", "targets": [],
        "solution": SOLUTION_SECRET});
    cli.queue(
        "practitioner",
        json!({"text": "Try this one.", "tool_calls": [tool_call("noesis_problem", problem)]}),
    );
    let harness = engine_harness(&home, cli.clone()).await;
    let created = harness.create(json!({"mode": "practice"})).await;
    harness.settled().await;
    let practitioner = agent_of(&created);
    let unranked = json!({"outcome": "solved_with_hints", "retest": false});
    let spoiled = format!("It was {SOLUTION_SECRET}.");
    cli.queue(
        "practitioner",
        json!({"text": spoiled, "tool_calls": [tool_call("noesis_conclude", unranked)]}),
    );
    let solved = json!({"outcome": "solved", "rung": 0, "retest": false});
    cli.queue(
        "practitioner",
        json!({"text": "Yes!", "tool_calls": [tool_call("noesis_conclude", solved)]}),
    );

    let sent = json!({"cmd": "noesis_send", "session": created.id, "text": "Is it 5?",
        "idempotency_key": "refused-conclusion"});
    harness.send_json(sent).await.expect("sent");
    harness.settled().await;
    assert_never_kept(&harness, practitioner, "It was");
    let transcript = serde_json::to_string(&harness.transcript(practitioner)).expect("json");
    assert!(transcript.contains("Yes!"), "{transcript}");
    assert_eq!(reminded(&cli), 1, "the retry was told why");
    let concluded = harness
        .events()
        .iter()
        .any(|event| matches!(event.kind, EventKind::NoesisPracticeConcluded { .. }));
    assert!(concluded, "the retry concluded the problem");
    harness.close().await;
}

/// A malformed attempt is neither shown nor kept, so its text cannot spoil
/// the question its retry poses.
#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_attempt_shows_nothing_and_its_retry_poses() {
    let home = Home::new();
    let cli = Arc::new(FauxCli::default());
    let broken = format!(r#"{{"text": "It is {KEY}.", "tool_calls": [{{"name": "noesis_ask""#);
    cli.queue("prober", Value::String(broken));
    cli.queue(
        "prober",
        json!({"text": "Your turn:", "tool_calls": [tool_call("noesis_ask", ask())]}),
    );
    let harness = engine_harness(&home, cli.clone()).await;
    let mut bus = harness.daemon.runtime.subscribe();
    let created = harness.create(json!({})).await;
    harness.settled().await;

    let prober = agent_of(&created);
    assert_eq!(live_text(&mut bus, prober), "");
    let session = harness.session(&created.id).await.expect("session");
    assert!(
        session.open_question.is_some(),
        "the retry's question opened"
    );
    assert_never_kept(&harness, prober, KEY);
    harness.close().await;
}
