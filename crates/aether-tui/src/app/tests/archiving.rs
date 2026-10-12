//! archiving reducer regressions.
use super::*;

/// `/clear` asks for the sweep, and `/show` remembers the
/// filter. The cleared agent's transcript and any
/// dangling selection are dropped the same way - on the event and the
/// tree refresh that follows it, not on the reply - so a second client's
/// archive reaches this client the same path a `/clear` this client
/// issued does.
#[test]
fn clear_sweeps_finished_work_and_the_filter_is_remembered() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (mut app, id) = app_with_agent();
    app.options.prefs_dir = Some(dir.path().to_path_buf());
    assert_eq!(app.selected, Some(id));

    let effects = app.run_command(SlashCommand::Clear);
    assert!(matches!(
        &effects[..],
        [Effect::Request {
            command: Command::Archive {
                target: ArchiveTarget::AllFinished,
                archived: true,
                idempotency_key: None,
            },
            ..
        }]
    ));

    assert!(
        app.transcripts.contains_key(&id),
        "the agent's transcript exists before the sweep"
    );
    app.reduce(AppEvent::Reply {
        then: Then::Archived,
        result: Ok(Reply::Agents(vec![agent(id, "worker")])),
    });
    assert!(
        app.transcripts.contains_key(&id),
        "the reply does not touch history"
    );
    assert_eq!(
        app.selected,
        Some(id),
        "the reply itself does not touch the selection - the event and the refresh do"
    );

    // The agent's own `agent.archived` event, arriving over the
    // subscription, is what actually clears its transcript.
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(Box::new(event(
        2,
        id,
        EventKind::AgentArchived { archived: true },
    )))));
    assert!(
        !app.transcripts.contains_key(&id),
        "the chat pane must not keep the cleared agent's transcript"
    );

    // The filter is a saved preference, and asking for the tree again
    // carries it.
    assert!(!app.show_archived);
    app.show_filter(Some(ShowFilter::Archived));
    assert!(app.show_archived);
    assert_eq!(
        crate::prefs::UiPrefs::load(dir.path())
            .expect("load")
            .show_archived,
        Some(true)
    );
    app.show_filter(Some(ShowFilter::Live));
    assert!(!app.show_archived);
    // A tree request is already in flight, so the refreshed one goes out
    // when that reply lands - and it carries the new filter.
    let effects = app.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok(Reply::Tree(tree(agent(id, "worker")))),
    });
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::Request {
            command: Command::GetTree {
                include_archived: false,
                ..
            },
            ..
        }
    )));
}

/// A selection dangles when the node
/// behind it leaves the tree for a reason other than this client's own
/// `/clear` - a second TUI, `aether archive <id>`, `--all-finished` run
/// from a shell. `apply_tree` must drop it on the next tree refresh
/// regardless of what caused the node to disappear.
#[test]
fn a_selection_removed_by_something_other_than_this_client_clears_on_the_next_refresh() {
    let (mut app, id) = app_with_agent();
    assert_eq!(app.selected, Some(id));

    // No `Then::Archived` reply at all here - this stands in for a
    // second client's archive, or `aether archive <id>` from a shell:
    // the only thing this client ever sees is the next tree.
    let empty = AgentTree {
        roots: vec![],
        nodes: std::collections::BTreeMap::new(),
    };
    app.reduce(AppEvent::Reply {
        then: Then::Tree,
        result: Ok(Reply::Tree(empty)),
    });
    assert_eq!(
        app.selected, None,
        "the node is gone from the tree, so the dangling selection must clear"
    );
}

/// `/show` is gone from the slash list, so every hint names the board's `.` key.
#[test]
fn cleared_work_flashes_point_at_the_board_key() {
    let (mut app, id) = app_with_agent();
    app.reduce(AppEvent::Reply {
        then: Then::Archived,
        result: Ok(Reply::Agents(vec![agent(id, "worker")])),
    });
    let flash = app.live_flash().unwrap_or_default().to_owned();
    assert!(flash.ends_with(". on the board shows it"), "{flash}");
    app.show_filter(None);
    let flash = app.live_flash().unwrap_or_default().to_owned();
    assert!(flash.ends_with(". on the board shows it"), "{flash}");
    assert!(!flash.contains("/show"), "{flash}");
}
