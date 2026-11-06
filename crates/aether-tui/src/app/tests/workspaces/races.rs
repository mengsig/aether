//! Workspace switches racing their own creations: every message reaches an
//! agent in the workspace the operator meant, whatever order replies land in.
use super::*;

mod intent;

/// Each create request in `effects`, with the `Then` its reply carries.
fn starts(effects: &[Effect]) -> Vec<(CreateAgent, Then)> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Request {
                command: Command::CreateAgent(req),
                then,
            } => Some((req.clone(), then.clone())),
            _ => None,
        })
        .collect()
}

/// The single create request in `effects`.
fn one_start(effects: &[Effect]) -> (CreateAgent, Then) {
    let mut all = starts(effects);
    assert_eq!(all.len(), 1, "{effects:?}");
    all.remove(0)
}

/// The daemon made the agent `start` asked for.
fn succeed(app: &mut App, start: &(CreateAgent, Then)) -> (AgentId, Vec<Effect>) {
    let mut made = agent(AgentId::new(), "assistant");
    made.scope = start.0.scope.clone().unwrap_or_default();
    let id = made.id;
    let effects = app.reduce(AppEvent::Reply {
        then: start.1.clone(),
        result: Ok(Reply::Agent(made)),
    });
    (id, effects)
}

fn fail(app: &mut App, start: &(CreateAgent, Then)) -> Vec<Effect> {
    app.reduce(AppEvent::Reply {
        then: start.1.clone(),
        result: Err(ClientError::Protocol(aether_protocol::ErrorReply::new(
            aether_protocol::ErrorCode::Unavailable,
            "daemon refused the root",
        ))),
    })
}

/// A model change settled: the agent now runs on `mock/changed`.
fn patched(app: &mut App, id: AgentId) -> Vec<Effect> {
    let mut agent = agent(id, "assistant");
    agent.model = ModelRef::new("mock", "changed");
    app.reduce(AppEvent::Reply {
        then: Then::PendingRootModelPatched(id),
        result: Ok(Reply::Agent(agent)),
    })
}

fn sent(effects: &[Effect]) -> Vec<(AgentId, String)> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Request {
                command: Command::SendMessage { to, content, .. },
                ..
            } => Some((*to, content.clone())),
            _ => None,
        })
        .collect()
}

fn submit(app: &mut App, text: &str) -> Vec<Effect> {
    type_text(app, text);
    app.reduce(key(KeyCode::Enter))
}

fn switch(app: &mut App, path: &Path) -> Vec<Effect> {
    submit(app, &format!("/workspace {}", path.display()))
}

/// Three directories, with a live root in the first, launched and viewing it.
fn live_in_first() -> (tempfile::TempDir, [PathBuf; 3], Agent, App) {
    let (tmp, a, b) = dirs();
    let c = crate::workspace::canonical(&{
        let dir = tmp.path().join("three");
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    });
    let live = root_in("live", &a, 10);
    let mut app = launched_in(&a);
    apply(&mut app, tree_of(vec![live.clone()]));
    assert_eq!(app.selected, Some(live.id));
    (tmp, [a, b, c], live, app)
}

#[test]
fn overlapping_switches_route_each_message_to_its_own_workspace() {
    for b_first in [true, false] {
        let (_tmp, [_, b, c], _, mut app) = live_in_first();
        let for_b = one_start(&switch(&mut app, &b));
        let for_c = one_start(&switch(&mut app, &c));
        assert_eq!(workspace_of(&for_c.0), Some(c.as_path()));
        assert!(sent(&submit(&mut app, "task for c")).is_empty());

        let (b_id, c_id, b_sent, c_sent);
        if b_first {
            (b_id, b_sent) = succeed(&mut app, &for_b);
            assert_eq!(app.selected, None, "b's reply must not take the view");
            (c_id, c_sent) = succeed(&mut app, &for_c);
        } else {
            (c_id, c_sent) = succeed(&mut app, &for_c);
            (b_id, b_sent) = succeed(&mut app, &for_b);
        }
        assert!(sent(&b_sent).is_empty(), "b_first={b_first}: {b_sent:?}");
        assert_eq!(sent(&c_sent), [(c_id, "task for c".into())]);
        assert_eq!(app.selected, Some(c_id), "b_first={b_first}");
        assert_ne!(b_id, c_id);
        assert_eq!(app.active_workspace(), Some(c.as_path()));
    }
}

/// B's creation failing does not disturb C's, still outstanding and wanted;
/// what was typed for B is said to be unsent, never sent elsewhere.
#[test]
fn a_failed_creation_leaves_another_outstanding_one_wanted() {
    let (_tmp, [_, b, c], _, mut app) = live_in_first();
    let for_b = one_start(&switch(&mut app, &b));
    submit(&mut app, "task for b");
    let for_c = one_start(&switch(&mut app, &c));
    submit(&mut app, "task for c");
    assert!(sent(&fail(&mut app, &for_b)).is_empty());
    assert!(
        app.live_flash()
            .is_some_and(|f| f.contains("1 message for") && f.contains("not sent")),
        "{:?}",
        app.live_flash()
    );
    assert_eq!(app.selected, None);
    let (c_id, effects) = succeed(&mut app, &for_c);
    assert_eq!(sent(&effects), [(c_id, "task for c".into())]);
    assert_eq!(app.selected, Some(c_id));
}

/// Leaving a failed start for a live root drops it, and says what it held.
#[test]
fn moving_on_from_a_failed_start_says_what_was_not_sent() {
    let (_tmp, [a, b, _], live, mut app) = live_in_first();
    let for_b = one_start(&switch(&mut app, &b));
    submit(&mut app, "task for b");
    fail(&mut app, &for_b);
    assert!(starts(&switch(&mut app, &a)).is_empty());
    assert_eq!(app.selected, Some(live.id));
    assert!(
        app.live_flash().is_some_and(|f| f.contains("not sent")),
        "{:?}",
        app.live_flash()
    );
    // B starts afresh, without the message dropped above.
    let again = one_start(&switch(&mut app, &b));
    let (b_id, effects) = succeed(&mut app, &again);
    assert!(sent(&effects).is_empty(), "{effects:?}");
    assert_eq!(app.selected, Some(b_id));
}

/// A live root that appeared where a start failed is used, not created twice;
/// what the failed start held is reported, never moved onto it.
#[test]
fn reopening_a_failed_start_workspace_uses_its_live_root() {
    let (_tmp, [_, b, _], live, mut app) = live_in_first();
    let for_b = one_start(&switch(&mut app, &b));
    submit(&mut app, "task for b");
    fail(&mut app, &for_b);
    let existing = root_in("other client", &b, 20);
    apply(&mut app, tree_of(vec![live, existing.clone()]));
    assert_eq!(app.selected, None, "a failed start keeps its wait");

    let effects = switch(&mut app, &b);
    assert!(starts(&effects).is_empty(), "{effects:?}");
    assert!(sent(&effects).is_empty(), "{effects:?}");
    assert_eq!(app.selected, Some(existing.id));
    assert!(
        app.live_flash()
            .is_some_and(|f| f.contains("1 message for") && f.contains("not sent")),
        "{:?}",
        app.live_flash()
    );
}

#[test]
fn picking_a_pending_workspace_again_starts_no_second_root() {
    let (_tmp, [_, b, _], _, mut app) = live_in_first();
    let for_b = one_start(&switch(&mut app, &b));
    assert!(starts(&switch(&mut app, &b)).is_empty(), "coalesced");
    submit(&mut app, "task for b");
    let (b_id, effects) = succeed(&mut app, &for_b);
    assert_eq!(sent(&effects), [(b_id, "task for b".into())]);
    assert_eq!(app.selected, Some(b_id));
}

#[test]
fn returning_to_a_live_root_keeps_it_when_the_pending_root_arrives() {
    let (_tmp, [a, b, _], live, mut app) = live_in_first();
    let for_b = one_start(&switch(&mut app, &b));
    assert!(sent(&submit(&mut app, "task for b")).is_empty());
    assert!(starts(&switch(&mut app, &a)).is_empty());
    assert_eq!(app.selected, Some(live.id));

    let (b_id, effects) = succeed(&mut app, &for_b);
    assert_eq!(app.selected, Some(live.id), "a late reply took the view");
    assert_eq!(sent(&effects), [(b_id, "task for b".into())]);
    assert_eq!(
        sent(&submit(&mut app, "task for a")),
        [(live.id, "task for a".into())]
    );
}

/// B and C each wait on a model change before their messages go out; each
/// change releases only its own agent's messages, in either reply order.
#[test]
fn model_changes_for_two_new_roots_release_only_their_own_messages() {
    for c_first in [true, false] {
        let (_tmp, [_, b, c], _, mut app) = live_in_first();
        let for_b = one_start(&switch(&mut app, &b));
        submit(&mut app, "/model mock/changed");
        submit(&mut app, "task for b");
        let (b_id, effects) = succeed(&mut app, &for_b);
        assert!(sent(&effects).is_empty(), "held for the model change");

        let for_c = one_start(&switch(&mut app, &c));
        submit(&mut app, "/model mock/changed");
        submit(&mut app, "task for c");
        let (c_id, effects) = succeed(&mut app, &for_c);
        assert!(sent(&effects).is_empty(), "held for the model change");

        let (b_sent, c_sent);
        if c_first {
            c_sent = patched(&mut app, c_id);
            b_sent = patched(&mut app, b_id);
        } else {
            b_sent = patched(&mut app, b_id);
            c_sent = patched(&mut app, c_id);
        }
        assert_eq!(
            sent(&c_sent),
            [(c_id, "task for c".into())],
            "c_first={c_first}"
        );
        assert_eq!(
            sent(&b_sent),
            [(b_id, "task for b".into())],
            "c_first={c_first}"
        );
        assert_eq!(app.selected, Some(c_id));
    }
}

/// B's model change fails and its messages stay held for B while C starts,
/// changes model and gets only its own.
#[test]
fn a_failed_model_change_keeps_its_messages_through_a_switch() {
    let (_tmp, [_, b, c], _, mut app) = live_in_first();
    let for_b = one_start(&switch(&mut app, &b));
    submit(&mut app, "/model mock/changed");
    submit(&mut app, "task for b");
    let (b_id, _) = succeed(&mut app, &for_b);
    app.reduce(AppEvent::Reply {
        then: Then::PendingRootModelPatched(b_id),
        result: Err(ClientError::Protocol(aether_protocol::ErrorReply::new(
            aether_protocol::ErrorCode::InvalidArgument,
            "model unavailable",
        ))),
    });
    assert!(
        app.live_flash()
            .is_some_and(|f| f.contains("queued messages were held"))
    );

    let for_c = one_start(&switch(&mut app, &c));
    submit(&mut app, "/model mock/changed");
    submit(&mut app, "task for c");
    let (c_id, _) = succeed(&mut app, &for_c);
    let effects = patched(&mut app, c_id);
    assert_eq!(sent(&effects), [(c_id, "task for c".into())]);

    // Back on B, `/model` retries its change, then its messages go out.
    app.select(b_id);
    let effects = submit(&mut app, "/model mock/changed");
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::Request { then: Then::PendingRootModelPatched(id), .. } if *id == b_id
    )));
    let effects = patched(&mut app, b_id);
    assert_eq!(sent(&effects), [(b_id, "task for b".into())]);
}

/// `--agent` picks the first view only: a later tree must not pull the
/// operator back from the workspace they switched to.
#[test]
fn cli_focus_does_not_override_a_later_workspace_switch() {
    let (_tmp, a, b) = dirs();
    let focused = root_in("focused", &a, 10);
    let mut app = launched_with(TuiOptions {
        cwd: Some(a.clone()),
        auto_create_root: true,
        focus: Some(focused.id),
        ..Default::default()
    });
    apply(&mut app, tree_of(vec![focused.clone()]));
    assert_eq!(app.selected, Some(focused.id));

    let for_b = one_start(&switch(&mut app, &b));
    apply(&mut app, tree_of(vec![focused.clone()]));
    assert_eq!(app.selected, None, "--agent reapplied");
    assert!(sent(&submit(&mut app, "task for b")).is_empty());
    let (b_id, effects) = succeed(&mut app, &for_b);
    assert_eq!(sent(&effects), [(b_id, "task for b".into())]);
}

/// After a failed creation the operator stays in the workspace they chose:
/// trees never adopt a root elsewhere, and each message retries there.
fn stays_after_failure(app: &mut App, start: (CreateAgent, Then), want: &Path, elsewhere: &Agent) {
    let mut start = start;
    fail(app, &start);
    for round in 0..2 {
        apply(app, tree_of(vec![elsewhere.clone()]));
        apply(app, tree_of(vec![elsewhere.clone()]));
        assert_eq!(app.selected, None, "round {round}: adopted another project");
        assert_eq!(app.active_workspace(), Some(want));
        let effects = submit(app, &format!("try {round}"));
        assert!(sent(&effects).is_empty(), "round {round}: {effects:?}");
        start = one_start(&effects);
        assert_eq!(workspace_of(&start.0), Some(want));
        if round == 0 {
            fail(app, &start);
            assert!(
                app.live_flash()
                    .is_some_and(|f| f.contains("daemon refused the root")),
                "{:?}",
                app.live_flash()
            );
        }
    }
    let (id, effects) = succeed(app, &start);
    assert_eq!(app.selected, Some(id));
    assert!(
        sent(&effects).iter().all(|(to, _)| *to == id),
        "{effects:?}"
    );
    assert!(
        sent(&effects).contains(&(id, "try 1".into())),
        "{effects:?}"
    );
}

#[test]
fn a_failed_startup_creation_keeps_the_launch_workspace() {
    let (_tmp, elsewhere_dir, here) = dirs();
    let elsewhere = root_in("elsewhere", &elsewhere_dir, 1);
    let mut app = launched_in(&here);
    let start = one_start(&apply(&mut app, tree_of(vec![elsewhere.clone()])));
    assert_eq!(workspace_of(&start.0), Some(here.as_path()));
    stays_after_failure(&mut app, start, &here, &elsewhere);
}

#[test]
fn a_timed_out_workspace_creation_keeps_its_workspace_and_socket_hint() {
    let (_tmp, elsewhere_dir, here) = dirs();
    let elsewhere = root_in("elsewhere", &elsewhere_dir, 1);
    let mut app = launched_in(&here);
    app.options.socket_path = Some("/private/operator's $socket".into());
    app.options.explicit_socket = true;
    let start = one_start(&apply(&mut app, tree_of(vec![elsewhere])));
    app.reduce(AppEvent::Reply {
        then: start.1,
        result: Err(ClientError::TimedOut {
            command: "create_agent".into(),
            timeout: std::time::Duration::from_secs(1),
            queries: None,
            host: None,
        }),
    });
    let message = app.live_flash().expect("creation failure");
    assert!(message.contains("connected but slow"), "{message}");
    assert!(
        message.contains("`aether --socket '/private/operator'\"'\"'s $socket' stop`"),
        "{message}"
    );
    assert_eq!(app.active_workspace(), Some(here.as_path()));
    let retry = one_start(&submit(&mut app, "try again"));
    assert_eq!(workspace_of(&retry.0), Some(here.as_path()));
}

#[test]
fn a_failed_switch_keeps_the_chosen_workspace() {
    let (_tmp, [_, b, _], live, mut app) = live_in_first();
    let start = one_start(&switch(&mut app, &b));
    stays_after_failure(&mut app, start, &b, &live);
}

/// A directory name that is not UTF-8 cannot be sent to the daemon or saved:
/// refused out loud, never swapped for its lossy look-alike.
#[cfg(unix)]
#[test]
fn a_non_utf8_workspace_is_refused_not_replaced() {
    use std::os::unix::ffi::OsStringExt;

    use crate::ui::views::workspace::{Sources, WorkspaceView};

    let (tmp, a, _) = dirs();
    let parent = crate::workspace::canonical(tmp.path());
    let raw = parent.join(std::ffi::OsString::from_vec(b"proj-\xff".to_vec()));
    let lookalike = parent.join("proj-\u{FFFD}");
    std::fs::create_dir(&raw).expect("mkdir raw");
    std::fs::create_dir(&lookalike).expect("mkdir lookalike");
    let prefs = tmp.path().join("prefs");
    let live = root_in("live", &a, 10);
    let mut app = launched_with(TuiOptions {
        cwd: Some(a.clone()),
        auto_create_root: true,
        prefs_dir: Some(prefs.clone()),
        ..Default::default()
    });
    apply(&mut app, tree_of(vec![live.clone()]));

    let sources = Sources {
        discovered: vec![raw.clone()],
        ..Default::default()
    };
    let view = WorkspaceView::from_sources(&sources, "|", &|_| None);
    app.bottom.push_view(Box::new(view));
    let effects = app.reduce(key(KeyCode::Enter));
    assert!(starts(&effects).is_empty(), "{effects:?}");
    assert!(
        app.live_flash().is_some_and(|f| f.contains("not UTF-8")),
        "{:?}",
        app.live_flash()
    );
    assert_eq!(app.selected, Some(live.id), "selection unchanged");
    assert_eq!(app.active_workspace(), Some(a.as_path()));

    assert!(
        starts(&switch(&mut app, &lookalike)).len() == 1,
        "the real one opens"
    );
    let saved = crate::prefs::UiPrefs::load(&prefs).expect("prefs");
    assert_eq!(saved.recent_workspaces, [lookalike.display().to_string()]);
}
