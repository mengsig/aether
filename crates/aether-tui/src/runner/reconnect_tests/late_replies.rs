//! Replies to requests a replaced connection made, which `Link::admit` lets
//! through: each must settle only what it was asked for.
use super::*;
use aether_core::agent::NetworkPolicy;
use aether_core::event::Event;
use aether_core::id::EventId;
use aether_protocol::Reply;

fn page_event(agent: AgentId, seq: u64, text: &str) -> Event {
    Event::from_draft(
        EventDraft::new(Actor::System, message(text)).agent(agent),
        EventId::new(),
        seq,
        now(),
    )
}

fn history_page(effects: Vec<Effect>, agent: AgentId) -> Then {
    let mut pages: Vec<_> = effects
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Request {
                command: Command::GetEvents { query, .. },
                then: then @ Then::Backfill { .. },
            } if query.agent == Some(agent) => Some(then),
            _ => None,
        })
        .collect();
    assert_eq!(pages.len(), 1, "exactly one history page for the agent");
    pages.pop().unwrap()
}

#[test]
fn reconnect_late_page_from_before_history_expired_must_not_take_the_new_page() {
    let mut app = crate::testing::app_with_transcript(&[]);
    let agent = app.selected.unwrap();
    app.transcripts.get_mut(&agent).unwrap().loaded = false;
    let old_page = history_page(app.reduce(AppEvent::Connected), agent);
    app.reduce(AppEvent::HistoryExpired);
    let new_page = history_page(app.reduce(AppEvent::Connected), agent);
    app.reduce(AppEvent::Reply {
        then: old_page,
        result: Ok(Reply::Events(vec![page_event(agent, 1, "before expiry")])),
    });
    assert!(
        !app.transcripts[&agent].loaded,
        "the expired connection's page is not the one in flight"
    );
    app.reduce(AppEvent::Reply {
        then: new_page,
        result: Ok(Reply::Events(vec![
            page_event(agent, 2, "after expiry"),
            page_event(agent, 1, "before expiry"),
        ])),
    });
    assert!(app.transcripts[&agent].loaded);
    let text = crate::testing::render_to(&app, 160, 40);
    assert!(text.contains("after expiry"), "{text}");
    assert!(text.contains("before expiry"), "{text}");
}

/// A first root waiting on the daemon's defaults, with no status to start it from.
fn root_waiting_on_defaults() -> (App, Then, aether_protocol::DaemonStatus, tempfile::TempDir) {
    let workspace = tempfile::tempdir().unwrap();
    let mut app = App::new(crate::app::TuiOptions {
        cwd: Some(workspace.path().to_path_buf()),
        ..Default::default()
    });
    crate::testing::apply_status_with_suspended(&mut app, 0);
    let mut before = app.status.take().unwrap();
    before.network_default = NetworkPolicy::Allowed;
    let effects = app.reduce(AppEvent::ExternalFinished {
        label: "login".into(),
        error: None,
    });
    let defaults = effects
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Request {
                command: Command::Status,
                then: then @ Then::RootDefaults { .. },
            } => Some(then),
            _ => None,
        })
        .expect("the root start reads the daemon's defaults first");
    app.reduce(AppEvent::Disconnected { because: None });
    app.reduce(AppEvent::Connected);
    (app, defaults, before, workspace)
}

fn created_network(effects: &[Effect]) -> Option<NetworkPolicy> {
    effects.iter().find_map(|effect| match effect {
        Effect::Request {
            command: Command::CreateAgent(create),
            ..
        } => Some(create.scope.as_ref().expect("a workspace scope").network),
        _ => None,
    })
}

#[test]
fn reconnect_late_root_defaults_must_not_replace_the_current_network_default() {
    let (mut app, defaults, before, _workspace) = root_waiting_on_defaults();
    crate::testing::apply_status_with_suspended(&mut app, 42);
    app.status.as_mut().unwrap().network_default = NetworkPolicy::Denied;
    let effects = app.reduce(AppEvent::Reply {
        then: defaults,
        result: Ok(Reply::Status(before)),
    });
    assert_eq!(app.status.as_ref().unwrap().agents_suspended, 42);
    assert_eq!(created_network(&effects), Some(NetworkPolicy::Denied));
}

#[test]
fn reconnect_late_root_defaults_before_the_new_status_reads_them_again() {
    let (mut app, defaults, before, _workspace) = root_waiting_on_defaults();
    assert!(
        app.status.is_none(),
        "the new connection's status has not landed"
    );
    let effects = app.reduce(AppEvent::Reply {
        then: defaults.clone(),
        result: Ok(Reply::Status(before)),
    });
    assert_eq!(created_network(&effects), None);
    let again = effects
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Request {
                command: Command::Status,
                then: then @ Then::RootDefaults { .. },
            } => Some(then),
            _ => None,
        })
        .expect("the defaults are read again on this connection");
    assert_ne!(again, defaults);
    crate::testing::apply_status_with_suspended(&mut app, 0);
    let mut current = app.status.take().unwrap();
    current.network_default = NetworkPolicy::Denied;
    let effects = app.reduce(AppEvent::Reply {
        then: again,
        result: Ok(Reply::Status(current)),
    });
    assert_eq!(created_network(&effects), Some(NetworkPolicy::Denied));
}
