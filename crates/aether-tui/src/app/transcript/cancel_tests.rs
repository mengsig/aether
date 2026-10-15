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

/// Stream `reasoning` and `text` under `invocation`, then end the agent the
/// way a budget or deadline sweep does: before its turn writes its records.
fn cut_by_a_sweep(feed: &mut Feed, root: AgentId, invocation: InvocationId, text: &str) {
    let started = feed.clock;
    let transcript = feed.transcripts.get_mut(&root).unwrap();
    let thinking = Delta::Thinking {
        text: "Weighing it".into(),
    };
    transcript.stream(invocation, &thinking, started);
    let text = Delta::Text { text: text.into() };
    transcript.stream(invocation, &text, started);
    feed.on(
        root,
        EventKind::AgentFailed {
            error: "token budget of 10: 11 used".into(),
        },
    );
}

fn assistant_texts(feed: &Feed, root: AgentId) -> Vec<String> {
    feed.models(root)
        .into_iter()
        .filter_map(|cell| match cell {
            CellModel::Assistant { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// The API path keeps a stopped reply as text for the next turn, so its
/// message lands after the sweep already drew the stream.
#[test]
fn a_reply_a_sweep_drew_is_not_repeated_by_its_message() {
    let thinking = ContentBlock::Thinking {
        text: "Weighing it".into(),
        signature: None,
        redacted: false,
    };
    let api = vec![ContentBlock::text("Cut answer")];
    let native = vec![thinking, ContentBlock::text("Cut answer")];
    for content in [api, native] {
        let mut feed = Feed::new();
        let (root, _) = feed.spawn(None, "aether", "assistant");
        let invocation = InvocationId::new();
        cut_by_a_sweep(&mut feed, root, invocation, "Cut answer");
        let before: Vec<_> = feed.models(root).into_iter().cloned().collect();
        feed.on(
            root,
            EventKind::AgentMessage {
                role: MessageRole::Assistant,
                content,
                invocation: Some(invocation),
            },
        );
        assert_eq!(feed.models(root), before.iter().collect::<Vec<_>>());
        assert_eq!(assistant_texts(&feed, root), ["Cut answer"]);
        assert!(feed.t(root).stream.is_empty());
    }
}

/// A native engine can still commit the whole message while it drains, and
/// that is the record: it is drawn even though the cut stream was.
#[test]
fn a_fuller_message_after_a_sweep_drew_its_stream_is_still_drawn() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let invocation = InvocationId::new();
    cut_by_a_sweep(&mut feed, root, invocation, "Cut ans");
    feed.on(
        root,
        EventKind::AgentMessage {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::text("Cut answer, finished.")],
            invocation: Some(invocation),
        },
    );
    assert_eq!(
        assistant_texts(&feed, root),
        ["Cut ans", "Cut answer, finished."]
    );
}

/// The stop lands mid-stream, so a few more deltas arrive after it. The
/// message that holds both parts adds nothing: only the tail is drawn.
#[test]
fn a_tail_streamed_after_a_sweep_is_drawn_once_when_its_message_lands() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let invocation = InvocationId::new();
    cut_by_a_sweep(&mut feed, root, invocation, "Cut answer ");
    let tail = Delta::Text {
        text: "and its tail".into(),
    };
    let at = feed.clock;
    feed.transcripts
        .get_mut(&root)
        .unwrap()
        .stream(invocation, &tail, at);
    feed.on(
        root,
        EventKind::AgentMessage {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::text("Cut answer and its tail")],
            invocation: Some(invocation),
        },
    );
    assert_eq!(assistant_texts(&feed, root), ["Cut answer", "and its tail"]);
    assert!(feed.t(root).stream.is_empty());
}

#[test]
fn a_cut_stream_is_released_once_its_message_cannot_come() {
    let mut feed = Feed::new();
    let (root, _) = feed.spawn(None, "aether", "assistant");
    let native = InvocationId::new();
    cut_by_a_sweep(&mut feed, root, native, "Cut native answer");
    assert!(feed.t(root).holds_cut());
    let tail = Delta::Text {
        text: " and its tail".into(),
    };
    let at = feed.clock;
    feed.transcripts
        .get_mut(&root)
        .unwrap()
        .stream(native, &tail, at);
    feed.on(
        root,
        EventKind::InvocationFailed {
            invocation: native,
            snapshot: aether_core::id::SnapshotId::new(),
            error: "cancelled".into(),
            class: Some(ModelErrorClass::Cancelled),
            retryable: false,
            cancelled: true,
            partial: vec![ContentBlock::text("Cut native answer and its tail")],
        },
    );
    assert_eq!(
        assistant_texts(&feed, root),
        ["Cut native answer", "and its tail"]
    );
    assert!(
        !feed.t(root).holds_cut(),
        "its last record left the cut held after drawing its tail"
    );

    cut_by_a_sweep(&mut feed, root, InvocationId::new(), "Cut API answer");
    assert!(feed.t(root).holds_cut());
    let next = Delta::Text {
        text: "A later turn".into(),
    };
    let at = feed.clock;
    let t = feed.transcripts.get_mut(&root).unwrap();
    t.stream(InvocationId::new(), &next, at);
    assert!(!t.holds_cut(), "a later turn's stream left the cut held");
}
