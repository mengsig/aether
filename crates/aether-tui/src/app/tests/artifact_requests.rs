//! artifact requests reducer regressions.
use super::*;
use crate::ui::views::ViewKind;

/// §9.3: a person reads names, not ids. The daemon writes `Task tsk_01J…
/// completed` because that is what it has; the client knows what the task
/// is, so the screen says that. An id it cannot place stays as the id.
#[test]
fn ids_the_daemon_wrote_read_as_names_on_screen() {
    let (mut app, id) = app_with_agent();
    let stranger = AgentId::new();
    let note = format!("handed off to {id}; {stranger} declined");
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        2,
        id,
        EventKind::AgentCancelled { reason: Some(note) },
    )))));
    let text = crate::testing::render_to(&app, 160, 40);
    assert!(
        text.contains("handed off to root"),
        "the id of a known agent should read as its name:\n{text}"
    );
    // Two ULIDs minted together share a long prefix, so this compares
    // whole ids: the stranger's survives, the known one is gone.
    assert!(
        text.contains(&stranger.to_string()),
        "an id nobody can place must stay as the id:\n{text}"
    );
    assert!(
        !text.contains(&id.to_string()),
        "the known id should be gone from the screen:\n{text}"
    );
}

/// Closing the pager, then `/artifacts` itself, before the bytes land and
/// reopening the list with nothing viewed: the late reply must not show
/// anything the person already walked away from.
#[test]
fn closing_the_artifacts_view_disarms_a_pending_artifact_request() {
    let (mut app, id) = app_with_agent();
    let mut record = artifact("still-loading.log");
    record.path = Some("/store/still-loading.log".into());
    let art = record.id;
    app.open_view(ViewKind::Artifacts);
    app.reduce(AppEvent::Reply {
        then: Then::Artifacts(id),
        result: Ok(Reply::Artifacts(vec![record.clone()])),
    });
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(
        effects.iter().any(|e| matches!(
            e,
            Effect::Request {
                command: Command::ReadArtifact { id, .. },
                ..
            } if *id == art
        )),
        "Enter must ask for the bytes: {effects:?}"
    );
    assert_eq!(app.artifacts.pending_bytes, Some((art, app.artifact_reads)));

    // Pager, then list, closed before the reply lands.
    app.reduce(key(KeyCode::Esc));
    app.reduce(key(KeyCode::Esc));
    assert!(app.bottom.view_stack.is_empty());
    app.selected = None;
    assert!(app.open_view(ViewKind::Artifacts).is_empty());
    assert_eq!(
        app.artifacts.pending_bytes, None,
        "reopening must disarm the old request"
    );

    // The reply for the old request lands late.
    app.reduce(AppEvent::Reply {
        then: crate::testing::artifact_bytes_then(&app, art),
        result: Ok(Reply::ArtifactBytes {
            artifact: record,
            base64: base64_of("too late"),
            truncated: false,
        }),
    });
    assert!(
        app.artifacts.open.is_none(),
        "a stale reply must not fill a pager that was closed"
    );
    assert!(
        app.bottom.top_view().is_some_and(|view| !view.fullscreen()),
        "a stale reply must not reopen the pager"
    );
}

/// `select` does not reset `/artifacts`, so a session that opens it on one
/// agent and then looks at another must not have the second agent's
/// artifact land in the first agent's list, nor move whose list it is.
#[test]
fn artifact_created_adopts_only_for_the_agent_the_list_shows() {
    let (mut app, a) = app_with_agent();
    let b = AgentId::new();
    let mut two = tree(agent(a, "root"));
    two.roots.push(b);
    two.nodes.insert(
        b,
        TreeNode {
            agent: agent(b, "helper"),
            children: vec![],
            task: None,
            waiting_on: vec![],
            lease: None,
            subtree_usage: Usage::default(),
            hidden_usage: None,
        },
    );
    app.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok(Reply::Tree(two)),
    });
    app.reduce(AppEvent::Reply {
        then: crate::testing::backfill_then(&app, b),
        result: Ok(Reply::Events(vec![])),
    });

    app.open_view(ViewKind::Artifacts);
    assert_eq!(app.artifacts.for_agent, Some(a));

    // Selection moves to B without closing the list.
    app.select(b);
    assert_eq!(app.selected, Some(b));
    assert_eq!(
        app.artifacts.for_agent,
        Some(a),
        "moving selection must not move whose list is open"
    );

    let record = artifact("b-output.txt");
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        2,
        b,
        EventKind::ArtifactCreated {
            artifact: record.clone(),
        },
    )))));

    assert_eq!(
        app.artifacts.for_agent,
        Some(a),
        "an event must never repoint for_agent"
    );
    assert!(
        app.artifacts.list.is_empty(),
        "B's artifact must not land in A's open list: {:?}",
        app.artifacts.list
    );
}
