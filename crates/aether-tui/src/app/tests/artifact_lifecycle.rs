//! artifact lifecycle reducer regressions.
use super::*;
use crate::ui::views::ViewKind;

fn reads(effects: &[Effect], art: aether_core::id::ArtifactId) -> bool {
    effects.iter().any(|e| {
        matches!(
            e,
            Effect::Request { command: Command::ReadArtifact { id, .. }, .. } if *id == art
        )
    })
}

fn pager_open(app: &App) -> bool {
    app.bottom.top_view().is_some_and(|view| view.fullscreen())
}

/// A finished tool call whose output names `art`, with no
/// `artifact.created` seen for it.
fn tool_output(app: &mut App, id: AgentId, art: aether_core::id::ArtifactId) {
    let call = ToolCall {
        id: ToolCallId::new(),
        provider_call_id: "toolu_2".into(),
        name: "bash".into(),
        input: serde_json::json!({"cmd": "cat huge.log"}),
        agent_id: id,
        task_id: None,
        invocation_id: None,
        resources: vec![],
        idempotency_key: None,
        requested_at: now(),
    };
    let call_id = call.id;
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        2,
        id,
        EventKind::ToolRequested {
            call,
            side_effect: SideEffectClass::ReadOnly,
        },
    )))));
    let outcome = ToolOutcome {
        artifacts: vec![art],
        truncated: true,
        ..ToolOutcome::ok("the head of the output\n[truncated]")
    };
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        3,
        id,
        EventKind::ToolSucceeded {
            call: call_id,
            outcome,
        },
    )))));
}

/// A session that has never listed this agent's artifacts has no cached
/// record to check, so the request must still go out and the daemon's own
/// answer, not the empty cache, decides: evicted bytes open no silent viewer.
#[test]
fn a_fresh_session_lets_the_reply_say_the_bytes_are_gone() {
    let (mut app, id) = app_with_agent();
    let mut record = artifact("huge.log");
    record.evicted_at = Some(now());
    let art = record.id;
    tool_output(&mut app, id, art);
    assert!(app.artifacts.list.is_empty(), "nothing is cached");

    let effects = app.open_view(ViewKind::ArtifactPager(art));
    assert!(
        reads(&effects, art),
        "an empty cache must not refuse to ask the daemon: {effects:?}"
    );
    app.reduce(AppEvent::Reply {
        then: crate::testing::artifact_bytes_then(&app, art),
        result: Ok(Reply::ArtifactBytes {
            artifact: record,
            base64: String::new(),
            truncated: false,
        }),
    });
    assert!(
        app.artifacts.open.is_none(),
        "an evicted reply must not fill the viewer"
    );
    assert!(
        app.live_flash().is_some_and(|n| n.contains("dropped")),
        "it must say why nothing opened: {:?}",
        app.live_flash()
    );
}

/// Once the reply has said the bytes are gone, the open pager must not go
/// on promising them.
#[test]
fn an_evicted_reply_does_not_leave_the_pager_downloading() {
    let (mut app, _) = app_with_agent();
    let mut record = artifact("huge.log");
    record.evicted_at = Some(now());
    let art = record.id;
    app.open_view(ViewKind::ArtifactPager(art));
    app.reduce(AppEvent::Reply {
        then: crate::testing::artifact_bytes_then(&app, art),
        result: Ok(Reply::ArtifactBytes {
            artifact: record,
            base64: String::new(),
            truncated: false,
        }),
    });
    let text = crate::testing::render_to(&app, 120, 40);
    assert!(
        !pager_open(&app) || !text.contains("downloading"),
        "the pager still waits for bytes the daemon said are gone:\n{text}"
    );
}

/// The record adopted from `artifact.created` is a snapshot of the moment
/// it was made; retention can drop the bytes after that without telling
/// the client. The reply, read fresh from the store, must decide.
#[test]
fn a_snapshot_adopted_from_artifact_created_does_not_outrank_the_reply() {
    let (mut app, id) = app_with_agent();
    let mut record = artifact("big.log");
    record.path = Some("/store/big.log".into());
    let art = record.id;
    tool_output(&mut app, id, art);
    app.open_view(ViewKind::Artifacts);
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        4,
        id,
        EventKind::ArtifactCreated {
            artifact: record.clone(),
        },
    )))));
    assert!(
        app.artifacts.list[0].has_bytes(),
        "the snapshot as created must look live"
    );

    let effects = app.reduce(key(KeyCode::Enter));
    assert!(
        reads(&effects, art),
        "the cheap pre-check must not block a record it believes has bytes: {effects:?}"
    );
    // Retention dropped the bytes after the snapshot was taken; the
    // daemon's own fresh record says so now.
    let mut evicted_now = record;
    evicted_now.evicted_at = Some(now());
    app.reduce(AppEvent::Reply {
        then: crate::testing::artifact_bytes_then(&app, art),
        result: Ok(Reply::ArtifactBytes {
            artifact: evicted_now,
            base64: String::new(),
            truncated: false,
        }),
    });
    assert!(
        app.artifacts.open.is_none(),
        "the fresh reply, not the stale snapshot, must decide"
    );
    assert!(
        app.live_flash().is_some_and(|n| n.contains("dropped")),
        "{:?}",
        app.live_flash()
    );
}

/// `/artifacts` is walked, not just listed, and an artifact whose bytes
/// retention has dropped says so instead of asking for them.
#[test]
fn the_artifacts_view_opens_what_the_cursor_is_on() {
    let (mut app, id) = app_with_agent();
    app.open_view(ViewKind::Artifacts);
    let mut kept = artifact("kept.log");
    kept.path = Some("/store/kept.log".into());
    let dropped = artifact("gone.log");
    let kept_id = kept.id;
    app.reduce(AppEvent::Reply {
        then: Then::Artifacts(id),
        result: Ok(Reply::Artifacts(vec![kept.clone(), dropped])),
    });
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(
        reads(&effects, kept_id),
        "Enter must open the artifact under the cursor: {effects:?}"
    );
    assert!(pager_open(&app));
    app.reduce(AppEvent::Reply {
        then: crate::testing::artifact_bytes_then(&app, kept_id),
        result: Ok(Reply::ArtifactBytes {
            artifact: kept.clone(),
            base64: base64_of("hello from the artifact"),
            truncated: false,
        }),
    });
    assert_eq!(
        app.artifacts.open.as_ref().map(|a| a.text.as_str()),
        Some("hello from the artifact")
    );
    let text = crate::testing::render_to(&app, 100, 30);
    assert!(text.contains("hello from the artifact"), "{text}");

    // Esc goes back to the list, not out of `/artifacts`.
    app.reduce(key(KeyCode::Esc));
    assert!(!pager_open(&app));
    assert_eq!(app.bottom.view_stack.len(), 1, "the list stays open");

    // A reply that lands after the pager was closed does not reopen it.
    app.reduce(AppEvent::Reply {
        then: crate::testing::artifact_bytes_then(&app, kept_id),
        result: Ok(Reply::ArtifactBytes {
            artifact: kept,
            base64: base64_of("too late"),
            truncated: false,
        }),
    });
    assert!(!pager_open(&app), "a stale reply must not reopen the pager");
    assert_eq!(
        app.artifacts.open.as_ref().map(|a| a.text.as_str()),
        Some("hello from the artifact"),
        "a reply nobody asked for must not replace the bytes"
    );

    // Bytes that retention dropped: say so, ask for nothing.
    app.reduce(key(KeyCode::Down));
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(effects.is_empty(), "{effects:?}");
    assert!(!pager_open(&app));
    assert!(
        app.live_flash().is_some_and(|n| n.contains("dropped")),
        "{:?}",
        app.live_flash()
    );
}
