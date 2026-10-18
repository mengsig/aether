//! Notes appended to a transcript: never a turn, and a `digest` note reaches
//! exactly the next turn's context while a `none` note reaches none.
use super::*;
use aether_core::model::ModelRequest;
use aether_core::note::{Note, NoteContext};

const SHOWN: &str = "DIGEST-NOTE 3 items shipped";
const HIDDEN: &str = "QUIET-NOTE for people only";

fn note(text: &str, context: NoteContext) -> Note {
    Note {
        text: text.into(),
        links: vec!["https://example.com/pr/1".into()],
        context,
        level: aether_core::note::NoteLevel::Info,
    }
}

fn starts(h: &Harness, agent: AgentId) -> usize {
    let started = types(h.store.as_ref(), agent);
    started
        .iter()
        .filter(|kind| *kind == "agent.started")
        .count()
}

fn system_text(request: &ModelRequest) -> String {
    let blocks: Vec<&str> = request.system.iter().map(|b| b.text.as_str()).collect();
    blocks.join("\n")
}

/// Send `text` and wait until the turn it starts has answered and parked.
async fn exchange(h: &Harness, agent: AgentId, text: &str) -> ModelRequest {
    let asked = h.client.requests().len();
    h.runtime
        .send_message(agent, vec![ContentBlock::text(text)], Actor::user())
        .expect("send");
    h.wait_for("the turn's model call", || {
        h.client.requests().len() > asked
    })
    .await;
    h.wait_until(agent, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    h.client.requests()[asked].clone()
}

async fn parked_agent(h: &Harness) -> AgentId {
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                config: Some(AgentConfig {
                    long_lived: true,
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..spec("watch the release queue")
            },
            Actor::user(),
        )
        .expect("create");
    h.wait_for("the first turn", || h.client.requests().len() == 1)
        .await;
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    agent.id
}

#[tokio::test]
async fn a_note_starts_no_turn_and_a_digest_note_reaches_only_the_next_one() {
    let h = harness(vec![
        ScriptedClient::text("watching"),
        ScriptedClient::text("noted"),
        ScriptedClient::text("still watching"),
    ])
    .await;
    let agent = parked_agent(&h).await;
    let (started, asked) = (starts(&h, agent), h.client.requests().len());

    for written in [
        note(SHOWN, NoteContext::Digest),
        note(HIDDEN, NoteContext::None),
    ] {
        let seq = h
            .runtime
            .append_note(agent, &written, Actor::user())
            .expect("append");
        let event = h.store.events(seq - 1, 1, &EventFilter::default()).unwrap();
        assert_eq!(Note::of(&event[0].kind), Some(written));
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(starts(&h, agent), started, "a note admitted a turn");
    assert_eq!(h.client.requests().len(), asked, "a note called the model");
    let status = h.store.agent(agent).unwrap().expect("agent").status;
    assert_eq!(status.kind(), AgentStatusKind::Waiting);

    let next = exchange(&h, agent, "anything new?").await;
    let system = system_text(&next);
    assert!(system.contains(SHOWN), "{system}");
    assert!(system.contains("https://example.com/pr/1"), "{system}");
    assert!(!system.contains(HIDDEN), "a `none` note reached the model");
    let messages = serde_json::to_string(&next.messages).unwrap();
    assert!(!messages.contains("DIGEST-NOTE"), "a note became a message");
    assert!(!messages.contains("QUIET-NOTE"), "a note became a message");

    let after = exchange(&h, agent, "and now?").await;
    let request = serde_json::to_string(&after).unwrap();
    assert!(!request.contains("DIGEST-NOTE"), "a note reached two turns");
    assert!(!request.contains("QUIET-NOTE"));
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn an_oversized_note_is_refused_with_the_limit_and_writes_nothing() {
    let h = harness(vec![ScriptedClient::text("watching")]).await;
    let agent = parked_agent(&h).await;
    let before = types(h.store.as_ref(), agent);

    let oversized = note(&"x".repeat(2049), NoteContext::Digest);
    let refused = h.runtime.append_note(agent, &oversized, Actor::user());
    let Err(aether_runtime::RuntimeError::Invalid(message)) = refused else {
        panic!("not refused as invalid: {refused:?}");
    };
    assert!(message.contains("2048"), "{message}");
    let unknown = h.runtime.append_note(
        AgentId::new(),
        &note("hi", NoteContext::None),
        Actor::user(),
    );
    assert!(
        matches!(unknown, Err(aether_runtime::RuntimeError::NotFound(_))),
        "{unknown:?}"
    );
    assert_eq!(types(h.store.as_ref(), agent), before);
    h.runtime.shutdown().await;
}
