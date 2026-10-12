#![allow(clippy::expect_used)]
use super::*;
use aether_client::{Client, Endpoint, SshRemote, Transport};
use aether_protocol::remote::{GitSummary, WorkspaceValidation};
use aether_protocol::{ServerFrame, Welcome};

fn remote_app() -> App {
    let mut app = crate::testing::app_with_transcript(&["hello"]);
    app.options.endpoint = Some(Endpoint::Ssh(
        SshRemote::new("box", "me@box").expect("remote"),
    ));
    app.options.instance_id = Some("original".into());
    app.options.auto_create_root = false;
    app
}

#[test]
fn host_paths_are_validated_without_touching_laptop_filesystem() {
    let mut app = remote_app();
    let path = std::path::Path::new("/host/only/workspace");
    let effects = app.open_workspace(path);
    assert!(
        matches!(&effects[..], [Effect::Request { command: Command::ValidateWorkspace { path }, then: Then::Host(host::HostRequest::Validate) }] if path == "/host/only/workspace")
    );
    let effects = app.reduce(AppEvent::Reply {
        then: Then::Host(host::HostRequest::Validate),
        result: Ok(Reply::WorkspaceValidation(WorkspaceValidation {
            valid: true,
            path: Some(path.into()),
            reason: None,
            git: None,
        })),
    });
    assert_eq!(app.workspace.as_deref(), Some(path));
    assert!(!effects.is_empty());
    assert!(app.options.command_hint().contains("--remote box"));
}

#[test]
fn expired_history_resubscribes_live_and_reloads_snapshots() {
    let mut app = remote_app();
    app.last_seq = Some(999);
    let queued_agent = *app.transcripts.keys().next().expect("agent");
    app.transcripts
        .get_mut(&queued_agent)
        .expect("transcript")
        .queue = vec!["preserve me".into()];
    assert!(app.reduce(AppEvent::HistoryExpired).is_empty());
    assert!(!app.connected);
    assert!(crate::testing::render_to(&app, 160, 30).contains("history expired"));
    let effects = app.on_connected();
    assert_eq!(app.last_seq, None);
    assert_eq!(app.transcripts[&queued_agent].queue, ["preserve me"]);
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::Subscribe { after: None }))
    );
    let snapshots = app.reduce(AppEvent::Subscribed {
        id: 1,
        last_seq: 1000,
    });
    assert!(snapshots.iter().any(|effect| matches!(
        effect,
        Effect::Request {
            command: Command::GetTree { .. },
            ..
        }
    )));
}

#[tokio::test]
async fn changed_daemon_discards_cursor_drafts_queue_and_stale_tree() {
    let mut app = remote_app();
    app.last_seq = Some(999);
    app.input.set_text("old draft");
    let (transport, mut peer) = Transport::memory(16);
    tokio::spawn(async move {
        let frame = peer.incoming.recv().await.expect("hello");
        peer.outgoing
            .send(ServerFrame::Reply {
                id: frame.id,
                result: Ok(Reply::Welcome(Welcome {
                    protocol_version: aether_protocol::PROTOCOL_VERSION,
                    daemon_version: "fixture".into(),
                    instance_id: "replacement".into(),
                    schema_version: 1,
                    pid: 1,
                    features: vec![],
                    daemon_protocol_version: None,
                    min_protocol_version: None,
                    idle_timeout_secs: None,
                    device: None,
                    submission_horizon: None,
                })),
            })
            .await
            .expect("welcome");
    });
    let client = Client::open(transport).await.expect("connection");
    let effects = app.reconnected(&client);
    assert_eq!(app.last_seq, None);
    assert!(app.input.text.is_empty() && app.tree.nodes.is_empty());
    assert!(app.options.queued.is_empty());
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::Subscribe { after: None }))
    );
    assert!(crate::testing::render_to(&app, 200, 30).contains("daemon identity changed"));
    assert!(
        effects.contains(&Effect::Outbox { settle: None }),
        "every remote connection reads what its outbox left unconfirmed"
    );
}

fn lost_send(key: &str) -> aether_client::Unresolved {
    let command = serde_json::from_value(serde_json::json!({
        "cmd": "send_message",
        "to": aether_core::id::AgentId::new(),
        "content": "deploy now",
        "idempotency_key": key,
    }))
    .expect("a keyed send");
    aether_client::Unresolved {
        command,
        horizon: None,
        sent_at: aether_core::time::now(),
        unconfirmed: Some("the host cannot say what became of a lost send".into()),
    }
}

fn waiting_on(unconfirmed: Vec<aether_client::Unresolved>) -> aether_client::Undecided {
    aether_client::Undecided {
        unconfirmed,
        unreadable: Vec::new(),
    }
}

#[test]
fn unconfirmed_sends_stay_on_the_status_line_until_each_is_decided() {
    use aether_client::Settlement;
    let mut app = remote_app();
    let waiting = vec![lost_send("ssh-1"), lost_send("ssh-2")];
    let effects = app.reduce(AppEvent::Outbox {
        settled: None,
        undecided: Ok(waiting_on(waiting.clone())),
    });
    assert!(effects.is_empty(), "reading the outbox blocks nothing");
    assert_eq!(app.undelivered, waiting);
    let screen = crate::testing::render_to(&app, 200, 30);
    assert!(screen.contains("2 unconfirmed /outbox"), "{screen}");
    assert!(screen.contains("may or may not have run"), "{screen}");

    app.reduce(AppEvent::Outbox {
        settled: Some((Settlement::Discard, Ok(None))),
        undecided: Ok(waiting_on(waiting[1..].to_vec())),
    });
    let screen = crate::testing::render_to(&app, 200, 30);
    assert!(screen.contains("1 unconfirmed /outbox"), "{screen}");
    assert!(screen.contains("discarded; nothing was sent"), "{screen}");

    app.reduce(AppEvent::Outbox {
        settled: Some((
            Settlement::SendAgain,
            Err(ClientError::DeliveryUnconfirmed("no answer".into())),
        )),
        undecided: Ok(waiting_on(waiting[1..].to_vec())),
    });
    assert_eq!(app.undelivered.len(), 1, "an unanswered resend stays");
    assert!(crate::testing::render_to(&app, 200, 30).contains("not settled"));

    app.reduce(AppEvent::Outbox {
        settled: Some((Settlement::MarkDelivered, Ok(None))),
        undecided: Ok(aether_client::Undecided::default()),
    });
    assert!(!crate::testing::render_to(&app, 200, 30).contains("unconfirmed /outbox"));
}

#[test]
fn git_summaries_and_unimplemented_services_have_visible_outcomes() {
    let mut app = remote_app();
    let path = PathBuf::from("/host/work");
    app.reduce(AppEvent::Reply {
        then: Then::Host(host::HostRequest::Git(path.clone())),
        result: Ok(Reply::GitSummary(GitSummary {
            repository: true,
            branch: Some("host-main".into()),
            dirty: true,
            ..Default::default()
        })),
    });
    assert_eq!(app.branch_at(&path), Some("host-main *"));
    app.reduce(AppEvent::Reply {
        then: Then::Host(host::HostRequest::Workspaces),
        result: Err(ClientError::Protocol(aether_protocol::ErrorReply {
            code: aether_protocol::ErrorCode::Unimplemented,
            message: "host services absent".into(),
        })),
    });
    assert!(crate::testing::render_to(&app, 200, 30).contains("host service unavailable"));
    assert!(app.host.workspaces.is_empty());
}

#[test]
fn remote_mentions_request_and_apply_only_host_workspace_files() {
    use aether_protocol::remote::{PathCompletion, PathCompletionKind, PathCompletions};
    let mut app = remote_app();
    let workspace = PathBuf::from("/host/only/workspace");
    app.workspace = Some(workspace.clone());
    app.selected = None;
    app.connected = true;
    app.input.set_text("@src");
    app.open_popup(crate::ui::bottom::popup::PopupKind::Mention(
        crate::ui::bottom::popup::MentionTab::Files,
    ));
    let effects = app.host_effects();
    assert!(effects.iter().any(|effect| matches!(effect, Effect::Request { command: Command::CompletePath { kind: PathCompletionKind::WorkspaceFile, prefix, workspace: Some(path), .. }, .. } if prefix == "src" && path == &workspace)));
    app.reduce(AppEvent::Reply {
        then: Then::Host(host::HostRequest::Files {
            workspace: workspace.clone(),
            prefix: "src".into(),
        }),
        result: Ok(Reply::PathCompletions(PathCompletions {
            entries: vec![PathCompletion {
                path: "src/host.rs".into(),
                is_dir: false,
            }],
            truncated: false,
        })),
    });
    assert_eq!(
        app.host.files.as_ref().expect("host files").2,
        ["src/host.rs"]
    );
    assert!(
        app.bottom
            .popup
            .as_ref()
            .expect("popup")
            .files
            .contains(&"src/host.rs".into())
    );
}
