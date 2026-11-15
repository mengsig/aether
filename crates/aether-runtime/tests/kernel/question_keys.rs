//! Questions keyed by seq: each is open until its own answer, and no count
//! caps how many one agent may ask.
use super::*;
use aether_runtime::{QUESTION_ANSWERED, pending_questions};

/// A started agent waiting on its first turn's question, with the run
/// further questions are recorded under.
async fn asker(h: &Harness) -> (AgentId, aether_core::id::RunId, u64) {
    let (agent, _) = h
        .runtime
        .create_agent(spec("paint it"), Actor::user())
        .expect("create");
    h.wait_until(agent.id, |s| matches!(s, AgentStatus::Waiting { .. }))
        .await;
    let first = pending_seqs(h, agent.id);
    assert_eq!(first.len(), 1, "the scripted turn asked once: {first:?}");
    (agent.id, agent.run, first[0])
}

fn ask(h: &Harness, agent: AgentId, run: aether_core::id::RunId, question: &str) -> u64 {
    let draft = EventDraft::new(
        Actor::Agent { id: agent },
        EventKind::QuestionAsked {
            question: question.into(),
        },
    )
    .agent(agent)
    .run(run);
    h.runtime.apply(Commit::new().event(draft)).expect("ask")[0].seq
}

fn pending_seqs(h: &Harness, agent: AgentId) -> Vec<u64> {
    pending_questions(h.store.as_ref(), agent)
        .expect("pending")
        .iter()
        .map(|asked| asked.seq)
        .collect()
}

fn reply(h: &Harness, agent: AgentId, seq: u64, text: &str) -> aether_runtime::Result<()> {
    h.runtime
        .reply_to_question(
            agent,
            seq,
            vec![ContentBlock::text(text)],
            Actor::user(),
            None,
        )
        .map(|_| ())
}

/// Every `question.answered` for `agent`: (question seq, targeted reply).
fn answers(h: &Harness, agent: AgentId) -> Vec<(u64, bool)> {
    h.store
        .events(0, 10_000, &EventFilter::own_events(agent))
        .expect("events")
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::Custom { name, payload } if name == QUESTION_ANSWERED => Some((
                payload["question_seq"].as_u64().expect("seq"),
                payload["reply"].as_bool().expect("reply"),
            )),
            _ => None,
        })
        .collect()
}

/// Turns that wait again whenever an answer wakes the agent, so it stays
/// live for the next reply.
fn waits_again(
    mut first: Vec<aether_model::testing::ScriptedResponse>,
) -> Vec<aether_model::testing::ScriptedResponse> {
    first.extend((0..4).map(|_| ScriptedClient::tool_call("wait_for", json!({}))));
    first
}

fn asking_harness() -> Vec<aether_model::testing::ScriptedResponse> {
    waits_again(vec![ScriptedClient::tool_call(
        "ask_user",
        json!({"question": "which colour?"}),
    )])
}

/// A reply addressed to one question answers it and no other.
#[tokio::test]
async fn a_reply_to_one_sequence_answers_only_that_question() {
    let h = harness(asking_harness()).await;
    let (agent, run, first) = asker(&h).await;
    let second = ask(&h, agent, run, "which shade?");
    assert_eq!(pending_seqs(&h, agent), [first, second]);

    reply(&h, agent, first, "blue").expect("the first is answerable");
    assert_eq!(pending_seqs(&h, agent), [second], "the second stays open");
    assert_eq!(answers(&h, agent), [(first, true)]);
    let again = reply(&h, agent, first, "red").expect_err("an answered question is closed");
    assert!(
        again.to_string().contains("not a pending question"),
        "{again}"
    );
    reply(&h, agent, second, "dark").expect("the second is answerable");
    assert!(pending_seqs(&h, agent).is_empty());
}

/// An answer that came before a question was asked is not its answer.
#[tokio::test]
async fn an_early_answer_to_one_question_does_not_answer_the_next() {
    let h = harness(asking_harness()).await;
    let (agent, run, first) = asker(&h).await;
    reply(&h, agent, first, "blue").expect("reply");
    let second = ask(&h, agent, run, "which shade?");
    assert_eq!(pending_seqs(&h, agent), [second]);
    assert!(aether_runtime::question_is_open(h.store.as_ref(), agent, second).expect("open"));
    assert_eq!(
        answers(&h, agent),
        [(first, true)],
        "nothing answered the second"
    );
}

/// A person's ordinary message answers every question pending when it
/// lands, each closed by its own event.
#[tokio::test]
async fn a_persons_message_closes_every_pending_question() {
    let h = harness(asking_harness()).await;
    let (agent, run, first) = asker(&h).await;
    let second = ask(&h, agent, run, "which shade?");
    h.runtime
        .send_message(agent, vec![ContentBlock::text("blue, dark")], Actor::user())
        .expect("send");
    assert!(pending_seqs(&h, agent).is_empty(), "both are answered");
    assert_eq!(answers(&h, agent), [(first, false), (second, false)]);
    let third = ask(&h, agent, run, "anything else?");
    assert_eq!(
        pending_seqs(&h, agent),
        [third],
        "a later question waits on its own"
    );
}

/// A colleague's message answers no question: only an operator does.
#[tokio::test]
async fn an_agents_message_answers_no_question() {
    let h = harness(asking_harness()).await;
    let (agent, _, first) = asker(&h).await;
    let colleague = AgentId::new();
    h.runtime
        .send_message(
            agent,
            vec![ContentBlock::text("fyi")],
            Actor::agent(colleague),
        )
        .expect("send");
    assert_eq!(pending_seqs(&h, agent), [first]);
    assert!(answers(&h, agent).is_empty());
}

/// History from before answers were recorded: an operator's message there
/// closed every earlier question, and still reads that way.
#[tokio::test]
async fn an_operator_message_without_answer_events_closes_earlier_questions() {
    let h = harness(asking_harness()).await;
    let (agent, run, first) = asker(&h).await;
    let message = aether_core::id::MessageId::new();
    let legacy = EventDraft::new(
        Actor::user(),
        EventKind::MessageSent {
            message,
            from: Actor::user(),
            to: agent,
            content: vec![ContentBlock::text("blue")],
        },
    )
    .agent(agent)
    .run(run);
    h.runtime
        .apply(Commit::new().event(legacy))
        .expect("legacy reply");
    assert!(
        pending_seqs(&h, agent).is_empty(),
        "question {first} was answered"
    );
    let second = ask(&h, agent, run, "which shade?");
    assert_eq!(pending_seqs(&h, agent), [second]);
}

/// C4: no count caps questions. The 21st and later questions from one agent,
/// all asked in one turn, are recorded and answerable like the first.
#[tokio::test]
async fn the_twenty_first_and_later_questions_still_go_through() {
    const ASKED: usize = 25;
    let questions: Vec<String> = (1..=ASKED)
        .map(|n| format!("question number {n}?"))
        .collect();
    let calls: Vec<(&str, serde_json::Value)> = questions
        .iter()
        .map(|q| ("ask_user", json!({ "question": q })))
        .collect();
    let h = harness(waits_again(vec![ScriptedClient::tool_calls(&calls, None)])).await;
    let (agent, _) = h
        .runtime
        .create_agent(spec("paint it"), Actor::user())
        .expect("create");
    let events = wait_until_events(h.store.as_ref(), agent.id, |events| {
        events
            .iter()
            .filter(|e| e.type_name() == "tool.succeeded")
            .count()
            >= ASKED
    })
    .await;
    let refused: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolSucceeded { outcome, .. } if outcome.is_error => Some(outcome),
            _ => None,
        })
        .collect();
    assert!(refused.is_empty(), "no question is refused: {refused:?}");
    let asked = pending_seqs(&h, agent.id);
    assert_eq!(asked.len(), ASKED, "every question is its own and pending");
    reply(&h, agent.id, asked[ASKED - 1], "the last one").expect("the 25th is answerable");
    reply(&h, agent.id, asked[20], "the 21st").expect("the 21st is answerable");
    assert_eq!(pending_seqs(&h, agent.id).len(), ASKED - 2);
}
