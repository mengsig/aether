use super::*;

#[test]
fn a_cancelled_api_partial_does_not_repeat_its_committed_reply() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let invocation = InvocationId::new();
    let content = vec![ContentBlock::text("Partial API answer")];
    feed.on(
        root,
        EventKind::AgentMessage {
            role: MessageRole::Assistant,
            content: content.clone(),
            invocation: Some(invocation),
        },
    );
    let before: Vec<_> = feed.models(root).into_iter().cloned().collect();
    feed.on(
        root,
        EventKind::InvocationFailed {
            invocation,
            snapshot: aether_core::id::SnapshotId::new(),
            error: "cancelled".into(),
            class: Some(ModelErrorClass::Cancelled),
            retryable: false,
            cancelled: true,
            partial: content,
        },
    );
    assert_eq!(feed.models(root), before.iter().collect::<Vec<_>>());
    assert_eq!(
        before
            .iter()
            .filter(|cell| matches!(cell, CellModel::Assistant { .. }))
            .count(),
        1
    );
    assert!(feed.alerts.is_empty());
}

#[test]
fn a_partial_recorded_after_a_stop_drew_its_stream_is_not_drawn_again() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let invocation = InvocationId::new();
    let started = feed.clock;
    feed.transcripts.get_mut(&root).unwrap().stream(
        invocation,
        &Delta::Text {
            text: "Cut answer".into(),
        },
        started,
    );
    feed.on(
        root,
        EventKind::AgentFailed {
            error: "token budget of 10: 11 used".into(),
        },
    );
    let before: Vec<_> = feed.models(root).into_iter().cloned().collect();
    feed.on(
        root,
        EventKind::InvocationFailed {
            invocation,
            snapshot: aether_core::id::SnapshotId::new(),
            error: "cancelled".into(),
            class: Some(ModelErrorClass::Cancelled),
            retryable: false,
            cancelled: true,
            partial: vec![ContentBlock::text("Cut answer")],
        },
    );
    assert_eq!(feed.models(root), before.iter().collect::<Vec<_>>());
    assert_eq!(
        before
            .iter()
            .filter(|cell| matches!(cell, CellModel::Assistant { text } if text == "Cut answer"))
            .count(),
        1
    );
}

#[test]
fn a_tail_streamed_after_a_stop_is_drawn_when_its_partial_lands() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let invocation = InvocationId::new();
    let text = |text: &str| Delta::Text { text: text.into() };
    let started = feed.clock;
    let t = feed.transcripts.get_mut(&root).unwrap();
    t.stream(invocation, &text("Cut "), started);
    feed.on(
        root,
        EventKind::AgentFailed {
            error: "deadline of 2026-10-15T00:00:00Z".into(),
        },
    );
    let t = feed.transcripts.get_mut(&root).unwrap();
    t.stream(invocation, &text("tail"), started);
    feed.on(
        root,
        EventKind::InvocationFailed {
            invocation,
            snapshot: aether_core::id::SnapshotId::new(),
            error: "cancelled".into(),
            class: Some(ModelErrorClass::Cancelled),
            retryable: false,
            cancelled: true,
            partial: vec![ContentBlock::text("Cut tail")],
        },
    );
    assert!(
        feed.t(root).stream.is_empty(),
        "the tail was left streaming"
    );
    let replies: Vec<_> = feed
        .models(root)
        .into_iter()
        .filter_map(|cell| match cell {
            CellModel::Assistant { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(replies, ["Cut", "tail"]);
}
