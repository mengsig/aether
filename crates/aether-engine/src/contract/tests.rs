use super::*;
use aether_core::{model::ModelRef, snapshot::ContextLayer};

fn snapshot() -> InvocationSnapshot {
    InvocationSnapshot {
        id: Default::default(),
        agent_id: Default::default(),
        task_id: None,
        turn: 1,
        model: ModelRef::new("claude-code", "sonnet"),
        adapter: None,
        capabilities: None,
        memory_ids: vec![],
        layers: vec![
            ContextLayer {
                kind: LayerKind::Checkpoint,
                text: Some("saved progress".into()),
                tokens: 4,
                items: 1,
                placement: LayerPlacement::System,
                memories: vec![],
            },
            ContextLayer {
                kind: LayerKind::RuntimeState,
                text: Some("child waiting".into()),
                tokens: 4,
                items: 1,
                placement: LayerPlacement::System,
                memories: vec![],
            },
        ],
        messages: vec![Message::user_text("old history")],
        tools: vec![],
        params: Default::default(),
        token_estimate: 8,
        created_at: aether_core::time::now(),
        memory_retrieval: None,
    }
}

#[test]
fn fresh_native_input_keeps_state_out_of_the_system_prompt() {
    let mut snapshot = snapshot();
    let prepared = prepare_user_prompt(
        &mut snapshot,
        "continue",
        HoistPlan {
            retained: None,
            nonce: "12345678",
        },
    );
    assert!(system_prompt(&snapshot).is_empty());
    assert!(prepared.prompt.contains("saved progress"));
    assert!(prepared.prompt.contains("child waiting"));
    assert!(prepared.prompt.ends_with("continue"));
    assert_eq!(snapshot.messages.len(), 1);
    assert!(!snapshot.messages[0].text().contains("old history"));
    assert!(
        snapshot
            .layers
            .iter()
            .all(|l| l.placement == LayerPlacement::UserTurn && l.tokens == 0)
    );
}

#[test]
fn retained_state_is_not_repeated_but_session_loss_restores_it() {
    let mut first = snapshot();
    let initial = prepare_user_prompt(
        &mut first,
        "continue",
        HoistPlan {
            retained: None,
            nonce: "12345678",
        },
    );
    let mut resumed = snapshot();
    let prepared = prepare_user_prompt(
        &mut resumed,
        "next",
        HoistPlan {
            retained: Some(&initial.sent),
            nonce: "abcdefgh",
        },
    );
    assert!(!prepared.prompt.contains("saved progress"));
    assert!(
        prepared.prompt.contains("child waiting"),
        "runtime status is refreshed"
    );
    assert!(system_prompt(&resumed).is_empty());
    assert_eq!(resumed.layers[0].placement, LayerPlacement::Retained);
    let restored = resend_everything(&mut resumed, &prepared);
    assert!(restored.contains("saved progress"));
    assert!(restored.ends_with("next"));
    assert_eq!(resumed.messages[0].text(), restored);
    assert_eq!(resumed.layers[0].placement, LayerPlacement::UserTurn);
    assert!(system_prompt(&resumed).is_empty());
}

#[test]
fn changing_state_does_not_change_the_native_system_prompt_on_resume() {
    let mut first = snapshot();
    let initial = prepare_user_prompt(
        &mut first,
        "first",
        HoistPlan {
            retained: None,
            nonce: "12345678",
        },
    );
    let mut second = snapshot();
    second.layers[0].text = Some("new checkpoint".into());
    second.layers[1].text = Some("child completed".into());
    let changed = prepare_user_prompt(
        &mut second,
        "second",
        HoistPlan {
            retained: Some(&initial.sent),
            nonce: "abcdefgh",
        },
    );
    assert_eq!(
        system_prompt(&first).as_bytes(),
        system_prompt(&second).as_bytes()
    );
    assert!(system_prompt(&second).is_empty());
    assert!(changed.prompt.contains("new checkpoint"));
    assert!(changed.prompt.contains("child completed"));
}

#[test]
fn no_state_sends_the_original_message_without_a_wrapper() {
    let mut snapshot = snapshot();
    snapshot.layers.clear();
    let prepared = prepare_user_prompt(
        &mut snapshot,
        "exact request",
        HoistPlan {
            retained: None,
            nonce: "12345678",
        },
    );
    assert_eq!(prepared.prompt, "exact request");
    assert_eq!(prepared.full_prompt, prepared.prompt);
    assert!(prepared.sent.is_empty());
    assert!(system_prompt(&snapshot).is_empty());
}
