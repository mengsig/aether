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

/// Expiry, not the link, retires a page: both pages here go out on one link.
#[test]
fn reconnect_page_asked_after_history_expired_is_not_taken_by_the_cancelled_one() {
    let mut app = crate::testing::app_with_transcript(&[]);
    let agent = app.selected.unwrap();
    app.transcripts.get_mut(&agent).unwrap().loaded = false;
    let cancelled = history_page(app.reduce(AppEvent::Connected), agent);
    app.reduce(AppEvent::HistoryExpired);
    app.selected = None;
    let current = history_page(app.select(agent), agent);
    app.reduce(AppEvent::Reply {
        then: cancelled,
        result: Ok(Reply::Events(vec![page_event(agent, 1, "cancelled page")])),
    });
    assert!(!app.transcripts[&agent].loaded);
    app.reduce(AppEvent::Connected);
    app.reduce(AppEvent::Reply {
        then: current,
        result: Ok(Reply::Events(vec![page_event(agent, 2, "current page")])),
    });
    assert!(app.transcripts[&agent].loaded);
    let text = crate::testing::render_to(&app, 160, 40);
    assert!(text.contains("current page"), "{text}");
    assert!(!text.contains("cancelled page"), "{text}");
}

/// A first root waiting on the daemon's defaults, with no status to start it
/// from, after its link was lost and before the next one is up.
fn root_waiting_on_defaults_before_connected()
-> (App, Then, aether_protocol::DaemonStatus, tempfile::TempDir) {
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
    (app, defaults, before, workspace)
}

/// The same root once the next link is up.
fn root_waiting_on_defaults() -> (App, Then, aether_protocol::DaemonStatus, tempfile::TempDir) {
    let (mut app, defaults, before, workspace) = root_waiting_on_defaults_before_connected();
    app.reduce(AppEvent::Connected);
    (app, defaults, before, workspace)
}

fn defaults_reads(effects: Vec<Effect>) -> Vec<Then> {
    effects
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Request {
                command: Command::Status,
                then: then @ Then::RootDefaults { .. },
            } => Some(then),
            _ => None,
        })
        .collect()
}

fn creates(effects: &[Effect]) -> usize {
    effects
        .iter()
        .filter(|effect| {
            matches!(
                effect,
                Effect::Request {
                    command: Command::CreateAgent(_),
                    ..
                }
            )
        })
        .count()
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

#[test]
fn reconnect_defaults_reread_before_connected_must_not_be_tagged_as_the_new_client() {
    let (mut app, old, before, _workspace) = root_waiting_on_defaults_before_connected();
    assert!(!app.connected);
    // The runner still holds the old link, so the old daemon answers what this sends.
    let over_old_link = app.reduce(AppEvent::Reply {
        then: old,
        result: Ok(Reply::Status(before.clone())),
    });
    assert_eq!(
        creates(&over_old_link),
        0,
        "nothing is created over a lost link"
    );
    let over_new_link = app.reduce(AppEvent::Connected);
    crate::testing::apply_status_with_suspended(&mut app, 42);
    app.status.as_mut().unwrap().network_default = NetworkPolicy::Denied;
    let current = app.status.clone().unwrap();
    let answers = defaults_reads(over_old_link)
        .into_iter()
        .map(|then| (then, before.clone()))
        .chain(
            defaults_reads(over_new_link)
                .into_iter()
                .map(|then| (then, current.clone())),
        );
    let mut effects = Vec::new();
    for (then, status) in answers {
        effects.extend(app.reduce(AppEvent::Reply {
            then,
            result: Ok(Reply::Status(status)),
        }));
    }
    assert_eq!(creates(&effects), 1);
    assert_eq!(created_network(&effects), Some(NetworkPolicy::Denied));
    assert_eq!(app.status.as_ref().unwrap().agents_suspended, 42);
}

#[test]
fn reconnect_root_start_resumes_once_after_its_link_dies_mid_reread() {
    let (mut app, old, before, _workspace) = root_waiting_on_defaults();
    let reread = defaults_reads(app.reduce(AppEvent::Reply {
        then: old,
        result: Ok(Reply::Status(before.clone())),
    }));
    assert_eq!(reread.len(), 1, "read again over the current link");
    app.reduce(AppEvent::Disconnected { because: None });
    let parked = app.reduce(AppEvent::Reply {
        then: reread[0].clone(),
        result: Ok(Reply::Status(before)),
    });
    assert!(
        parked.is_empty(),
        "nothing goes out over the lost link: {parked:?}"
    );
    let resumed = defaults_reads(app.reduce(AppEvent::Connected));
    assert_eq!(resumed.len(), 1, "resumed once on the new link");
    crate::testing::apply_status_with_suspended(&mut app, 0);
    let mut current = app.status.take().unwrap();
    current.network_default = NetworkPolicy::Denied;
    let effects = app.reduce(AppEvent::Reply {
        then: resumed[0].clone(),
        result: Ok(Reply::Status(current)),
    });
    assert_eq!(creates(&effects), 1);
    assert_eq!(created_network(&effects), Some(NetworkPolicy::Denied));
    app.reduce(AppEvent::Disconnected { because: None });
    let again = app.reduce(AppEvent::Connected);
    assert_eq!(creates(&again), 0);
    assert!(
        defaults_reads(again).is_empty(),
        "no start is resumed twice"
    );
}

fn bytes_route(effects: Vec<Effect>, artifact: aether_core::id::ArtifactId) -> Then {
    effects
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Request {
                command: Command::ReadArtifact { id, .. },
                then,
            } if id == artifact => Some(then),
            _ => None,
        })
        .expect("the pager reads the artifact's bytes")
}

fn bytes_reply(then: Then, artifact: &aether_core::artifact::Artifact, text: &str) -> AppEvent {
    use base64::Engine as _;
    AppEvent::Reply {
        then,
        result: Ok(Reply::ArtifactBytes {
            artifact: artifact.clone(),
            base64: base64::engine::general_purpose::STANDARD.encode(text),
            truncated: false,
        }),
    }
}

fn retained_artifact() -> aether_core::artifact::Artifact {
    aether_core::artifact::Artifact {
        id: aether_core::id::ArtifactId::new(),
        kind: aether_core::artifact::ArtifactKind::CommandOutput,
        name: "retained.log".into(),
        path: Some("/store/retained.log".into()),
        summary: String::new(),
        size_bytes: 15,
        content_type: None,
        digest: None,
        created_by: Actor::System,
        agent_id: None,
        task_id: None,
        created_at: now(),
        evicted_at: None,
    }
}

/// The first read's reply lands after the pager was reopened, then the second
/// read finds `record` evicted: only the second may settle the pager.
fn first_read_lands_after_the_reopen(
    app: &mut App,
    mut record: aether_core::artifact::Artifact,
    first: Then,
    second: Then,
) {
    assert_ne!(first, second, "each read has its own route");
    app.reduce(bytes_reply(first, &record, "before eviction"));
    assert!(
        app.artifacts.pending_bytes.is_some(),
        "the reopened read still waits"
    );
    assert!(
        app.artifacts.open.is_none(),
        "the first read's bytes are not shown"
    );
    record.evicted_at = Some(now());
    record.path = None;
    app.reduce(bytes_reply(second, &record, ""));
    let pager_open = app.bottom.top_view().is_some_and(|view| view.fullscreen());
    assert!(
        !pager_open,
        "the current read's eviction reply closes the pager"
    );
    assert!(
        app.live_flash()
            .is_some_and(|notice| notice.contains("dropped"))
    );
}

#[test]
fn reopening_a_pager_in_one_connection_is_not_settled_by_the_first_read() {
    use crate::ui::views::ViewKind;
    let mut app = crate::testing::app_with_transcript(&[]);
    let record = retained_artifact();
    let first = bytes_route(app.open_view(ViewKind::ArtifactPager(record.id)), record.id);
    app.reduce(AppEvent::Key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Esc,
    )));
    let second = bytes_route(app.open_view(ViewKind::ArtifactPager(record.id)), record.id);
    first_read_lands_after_the_reopen(&mut app, record, first, second);
}

#[test]
fn reconnect_read_asked_while_disconnected_does_not_settle_a_pager_reopened_after() {
    use crate::ui::views::ViewKind;
    let mut app = crate::testing::app_with_transcript(&[]);
    let record = retained_artifact();
    app.reduce(AppEvent::Disconnected { because: None });
    let first = bytes_route(app.open_view(ViewKind::ArtifactPager(record.id)), record.id);
    app.reduce(AppEvent::Connected);
    app.reduce(AppEvent::Key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Esc,
    )));
    let second = bytes_route(app.open_view(ViewKind::ArtifactPager(record.id)), record.id);
    first_read_lands_after_the_reopen(&mut app, record, first, second);
}

#[test]
fn reconnect_old_artifact_read_must_not_hide_a_new_eviction_reply() {
    use crate::ui::views::ViewKind;
    let mut app = crate::testing::app_with_transcript(&[]);
    let mut record = retained_artifact();
    let old = bytes_route(app.open_view(ViewKind::ArtifactPager(record.id)), record.id);
    app.reduce(AppEvent::Disconnected { because: None });
    app.reduce(AppEvent::Connected);
    app.reduce(AppEvent::Key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Esc,
    )));
    let new = bytes_route(app.open_view(ViewKind::ArtifactPager(record.id)), record.id);
    app.reduce(bytes_reply(old, &record, "before eviction"));
    record.evicted_at = Some(now());
    record.path = None;
    app.reduce(bytes_reply(new, &record, ""));
    let pager_open = app.bottom.top_view().is_some_and(|view| view.fullscreen());
    assert!(
        !pager_open,
        "the current read's eviction reply closes the pager"
    );
    assert!(
        app.live_flash()
            .is_some_and(|notice| notice.contains("dropped"))
    );
}

/// Over A, then B, then A again: the view was rebuilt for B, so a reply to
/// what A was asked first must not settle what A is asked now.
#[tokio::test]
async fn reconnect_identity_round_trip_drops_replies_from_before_the_rebuild() {
    use crate::ui::views::ViewKind;
    let mut original = Fixture::new().await;
    let mut replacement = Fixture::new().await;
    let mut app = App::new(TuiOptions {
        auto_create_root: false,
        ..Default::default()
    });
    let old_link = original.reconnect(&mut app).await;
    let original_instance = app.options.instance_id.clone();
    let record = retained_artifact();
    let first = bytes_route(app.open_view(ViewKind::ArtifactPager(record.id)), record.id);
    app.reduce(AppEvent::Disconnected { because: None });
    let middle_link = replacement.reconnect_after(&mut app, &old_link).await;
    assert_ne!(
        app.options.instance_id, original_instance,
        "another database"
    );
    app.reduce(AppEvent::Disconnected { because: None });
    let current_link = original.reconnect_after(&mut app, &middle_link).await;
    assert_eq!(
        app.options.instance_id, original_instance,
        "the first again"
    );
    let second = bytes_route(app.open_view(ViewKind::ArtifactPager(record.id)), record.id);
    let mut admitted = Vec::new();
    for (link, then) in [(&old_link, first), (&current_link, second)] {
        let LoopEvent::AppFrom {
            instance,
            generation,
            event,
        } = protocol_event(link, bytes_reply(then, &record, "retained"))
        else {
            panic!("a scoped reply");
        };
        admitted.push(current_link.admit(&instance, generation, event).is_some());
    }
    original.stop().await;
    replacement.stop().await;
    assert_eq!(
        admitted,
        [false, true],
        "only the current read's reply reaches the view"
    );
}
