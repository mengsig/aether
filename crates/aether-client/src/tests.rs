use aether_core::event::Event;
use aether_core::id::{AgentId, InvocationId, TaskId};
use aether_protocol::{
    ClientFrame, Command, DaemonStatus, Delta, ErrorCode, ErrorReply, EventQuery, Hello, Reply,
    ServerFrame, Welcome,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use super::*;

mod subscription_queue;
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::EventId;
use aether_core::model::Usage;
use aether_core::time::now;
use aether_protocol::PROTOCOL_VERSION;

pub(super) fn welcome() -> Welcome {
    Welcome {
        protocol_version: PROTOCOL_VERSION,
        daemon_version: "test".into(),
        instance_id: "inst".into(),
        schema_version: 1,
        pid: 42,
        features: vec![],
        daemon_protocol_version: None,
        min_protocol_version: None,
        idle_timeout_secs: None,
        device: None,
        submission_horizon: None,
    }
}

fn status(pid: u32) -> DaemonStatus {
    DaemonStatus {
        problems: Vec::new(),
        instance_id: "inst".into(),
        version: "test".into(),
        pid,
        started_at: now(),
        last_seq: 0,
        agents_live: 0,
        agents_running: 0,
        agents_waiting: 0,
        agents_blocked: 0,
        agents_suspended: 0,
        agents_paused: 0,
        tasks_active: 0,
        pending_approvals: 0,
        default_model: None,
        usage_total: Usage::default(),
        yolo: false,
        engines: Vec::new(),
        providers: Vec::new(),
        resource_leases: Vec::new(),
        runs: Vec::new(),
        approval_default: Default::default(),
        network_default: Default::default(),
        max_concurrent_turns: None,
        max_concurrent_engine_turns: None,
        engine_turns_running: 0,
        run_budget_usd: None,
        run_hard_stop_usd: None,
        run_hard_stop_tokens: None,
        mcp: Vec::new(),
        low_disk: None,
        writes: None,
        bridge: None,
    }
}

/// A fake daemon that answers `Hello` and hands remaining frames to `handler`.
fn fake_daemon(
    mut peer: MemoryPeer,
    handler: impl Fn(
        ClientFrame,
        &mpsc::Sender<ServerFrame>,
    ) -> futures::future::BoxFuture<'static, bool>
    + Send
    + 'static,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(frame) = peer.incoming.recv().await {
            if let Command::Hello(_) = frame.command {
                let _ = peer
                    .outgoing
                    .send(ServerFrame::Reply {
                        id: frame.id,
                        result: Ok(Reply::Welcome(welcome())),
                    })
                    .await;
                continue;
            }
            if !handler(frame, &peer.outgoing).await {
                break;
            }
        }
    })
}

#[tokio::test]
async fn artifact_publication_refuses_a_daemon_without_the_feature() {
    let (transport, peer) = Transport::memory(16);
    let _daemon = fake_daemon(peer, |_, _| Box::pin(async { true }));
    let client = Client::open(transport).await.expect("open");
    let error = client
        .publish_artifact(aether_protocol::PublishArtifact {
            task: TaskId::new(),
            name: "evidence".into(),
            summary: "test".into(),
            content_type: "text/plain".into(),
            text: "observed".into(),
        })
        .await
        .expect_err("old daemon must be refused before submission");
    assert!(matches!(error, ClientError::Incompatible(_)));
}

#[tokio::test]
async fn a_named_checkout_is_refused_by_a_daemon_without_the_feature() {
    let (transport, peer) = Transport::memory(16);
    let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = sent.clone();
    let _daemon = fake_daemon(peer, move |_, _| {
        seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { true })
    });
    let client = Client::open(transport).await.expect("open");
    let error = client
        .create_agent(aether_protocol::CreateAgent {
            role: "worker".into(),
            repository_root: Some("/src/widget".into()),
            ..Default::default()
        })
        .await
        .expect_err("old daemon must be refused before submission");
    let ClientError::Incompatible(message) = error else {
        panic!("expected Incompatible: {error:?}");
    };
    assert!(message.contains("repository_root"), "{message}");
    assert_eq!(sent.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_named_build_cache_is_refused_by_a_daemon_without_the_feature() {
    let (transport, peer) = Transport::memory(16);
    let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = sent.clone();
    let _daemon = fake_daemon(peer, move |_, _| {
        seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { true })
    });
    let client = Client::open(transport).await.expect("open");
    let config = aether_core::agent::AgentConfig {
        build_cache: Some("/srv/cache/items/one".into()),
        ..Default::default()
    };
    let error = client
        .create_agent(aether_protocol::CreateAgent {
            role: "worker".into(),
            config: Some(config),
            ..Default::default()
        })
        .await
        .expect_err("old daemon must be refused before submission");
    let ClientError::Incompatible(message) = error else {
        panic!("expected Incompatible: {error:?}");
    };
    assert!(message.contains("build_cache"), "{message}");
    assert_eq!(sent.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn repository_aliases_are_refused_by_a_daemon_without_the_feature() {
    let (transport, peer) = Transport::memory(16);
    let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = sent.clone();
    let _daemon = fake_daemon(peer, move |_, _| {
        seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { true })
    });
    let client = Client::open(transport).await.expect("open");
    let aliased = client
        .alias_repository(
            "archon".into(),
            None,
            Some("github.com/mengsig/archon".into()),
            true,
        )
        .await
        .expect_err("old daemon must be refused before submission");
    assert!(
        matches!(aliased, ClientError::Incompatible(_)),
        "{aliased:?}"
    );
    let listed = client
        .repository_aliases()
        .await
        .expect_err("old daemon must be refused before submission");
    assert!(matches!(listed, ClientError::Incompatible(_)), "{listed:?}");
    assert_eq!(sent.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_note_is_refused_by_a_daemon_without_the_feature() {
    let (transport, peer) = Transport::memory(16);
    let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = sent.clone();
    let _daemon = fake_daemon(peer, move |_, _| {
        seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { true })
    });
    let client = Client::open(transport).await.expect("open");
    let note = aether_core::note::Note {
        text: "3 items shipped".into(),
        links: Vec::new(),
        context: aether_core::note::NoteContext::None,
        level: aether_core::note::NoteLevel::Info,
    };
    let error = client
        .append_note(aether_core::id::AgentId::new(), note)
        .await
        .expect_err("old daemon must be refused before submission");
    let ClientError::Protocol(refusal) = error else {
        panic!("expected a protocol refusal: {error:?}");
    };
    assert_eq!(refusal.code, aether_protocol::ErrorCode::Unimplemented);
    assert!(refusal.message.contains("`notes`"), "{}", refusal.message);
    assert_eq!(sent.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_daemon_without_notifications_is_skipped_and_never_sent_one() {
    let (transport, peer) = Transport::memory(16);
    let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = sent.clone();
    let _daemon = fake_daemon(peer, move |_, _| {
        seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { true })
    });
    let client = Client::open(transport).await.expect("open");
    assert!(!client.serves_notifications());
    let error = client
        .notifications(Vec::new(), None)
        .await
        .expect_err("old daemon must be refused before submission");
    let ClientError::Protocol(refusal) = error else {
        panic!("expected a protocol refusal: {error:?}");
    };
    assert_eq!(refusal.code, aether_protocol::ErrorCode::Unimplemented);
    assert!(
        refusal.message.contains("`notifications`"),
        "{}",
        refusal.message
    );
    assert_eq!(sent.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn handshake_succeeds() {
    let (transport, peer) = Transport::memory(16);
    let _daemon = fake_daemon(peer, |_, _| Box::pin(async { true }));
    let client = Client::open(transport).await.expect("open");
    assert_eq!(client.welcome().map(|w| w.pid), Some(42));
    assert!(client.is_connected());
}

/// A refused hello after an update has this shape: the daemon still running
/// is from the build before an update, and its refusal names the window
/// it serves but not which side is stale. The other side is added here,
/// because only the refused client knows its own version.
#[tokio::test]
async fn incompatible_daemon_is_reported_with_this_builds_version() {
    let (transport, mut peer) = Transport::memory(16);
    tokio::spawn(async move {
        if let Some(frame) = peer.incoming.recv().await {
            let _ = peer
                .outgoing
                .send(ServerFrame::Reply {
                    id: frame.id,
                    result: Err(ErrorReply::new(ErrorCode::Incompatible, "protocol 1 vs 2")),
                })
                .await;
        }
    });
    let err = Client::open(transport).await.unwrap_err();
    let ClientError::Incompatible(message) = err else {
        panic!("a refused hello must be Incompatible: {err:?}");
    };
    assert!(
        message.contains("protocol 1 vs 2"),
        "the daemon's own words are kept: {message}"
    );
    assert!(
        message.contains(&aether_protocol::PROTOCOL_VERSION.to_string()),
        "this build's protocol version names the other side of the skew: {message}"
    );
    assert!(
        message.contains("from a different build"),
        "which side is stale is named, since the daemon's words do not: {message}"
    );
}

/// Replies correlate strictly by id, so a daemon that answers at the
/// wrong one - an older build that cannot decode the frame at all and
/// replies at `id: 0` - resolves nothing. Without a bound the caller
/// waits for ever, which is `aether memory forget` against a daemon left
/// running from a previous build.
#[tokio::test]
async fn a_reply_that_never_arrives_fails_the_request_instead_of_hanging() {
    let (transport, peer) = Transport::memory(16);
    let _daemon = fake_daemon(peer, |_, out| {
        let out = out.clone();
        Box::pin(async move {
            // An older daemon's answer to a frame it could not decode.
            let _ = out
                .send(ServerFrame::Reply {
                    id: 0,
                    result: Err(ErrorReply::invalid("bad frame: unknown variant")),
                })
                .await;
            true
        })
    });
    let client = Client::open(transport)
        .await
        .expect("open")
        .with_request_timeout(Duration::from_millis(200));
    let err = client
        .request(Command::ForgetMemory {
            id: aether_core::id::MemoryId::new(),
            reason: None,
        })
        .await
        .expect_err("the request is failed, not left waiting");
    let ClientError::TimedOut { command, .. } = err else {
        panic!("{err:?}");
    };
    assert_eq!(command, "forget_memory", "the error names what was sent");
    assert!(client.is_connected(), "the connection itself is fine");
}

/// The MCP bridge's `EngineTool` call blocks on the operator's approval
/// verdict by design, so it must opt out of the blanket request timeout:
/// an operator who takes longer than the bound to answer must still have
/// their answer land, not be told the tool call failed.
#[tokio::test]
async fn a_reply_answered_after_a_short_timeout_still_succeeds_once_the_client_opts_out() {
    let short = Duration::from_millis(40);
    let (transport, peer) = Transport::memory(16);
    let _daemon = fake_daemon(peer, move |frame, out| {
        let out = out.clone();
        Box::pin(async move {
            // The operator answering after what would be a bound
            // client's deadline - the ordinary shape of a person taking
            // their time on an approval card.
            tokio::time::sleep(short * 3).await;
            let _ = out
                .send(ServerFrame::Reply {
                    id: frame.id,
                    result: Ok(Reply::Ok),
                })
                .await;
            true
        })
    });
    let client = Client::open(transport)
        .await
        .expect("open")
        .without_request_timeout();
    let reply = client.request(Command::Status).await;
    assert!(
        reply.is_ok(),
        "a client with no request bound must wait for a late but real answer: {reply:?}"
    );
}

/// Without the fix above, the bridge's client keeps the default bound and
/// this same late answer is lost: the request fails and, since nothing
/// still holds the receiver, there is nothing left to deliver the
/// operator's verdict to.
#[tokio::test]
async fn the_same_late_reply_times_out_a_client_still_bound_by_a_request_timeout() {
    let short = Duration::from_millis(40);
    let (transport, peer) = Transport::memory(16);
    let _daemon = fake_daemon(peer, move |frame, out| {
        let out = out.clone();
        Box::pin(async move {
            tokio::time::sleep(short * 3).await;
            let _ = out
                .send(ServerFrame::Reply {
                    id: frame.id,
                    result: Ok(Reply::Ok),
                })
                .await;
            true
        })
    });
    let client = Client::open(transport)
        .await
        .expect("open")
        .with_request_timeout(short);
    let err = client
        .request(Command::Status)
        .await
        .expect_err("a bound client gives up before the late answer arrives");
    assert!(matches!(err, ClientError::TimedOut { .. }), "{err:?}");
}

/// A late reply after a timeout cannot be delivered - the caller's
/// receiver is already dropped - so it must at least be visible, naming
/// the request it answers, rather than vanish as an id nobody recognizes.
#[test]
fn a_reply_after_timeout_is_logged_by_command_not_silently_dropped() {
    // Tracing's callsite interest is process-wide; isolate the subscriber
    // from reader loops in concurrently running tests.
    let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "tests::late_reply_log_fixture",
            "--ignored",
            "--nocapture",
        ])
        .output()
        .expect("run logging fixture");
    assert!(
        output.status.success(),
        "logging fixture failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test(start_paused = true)]
#[ignore = "subprocess fixture, launched by the logging test"]
async fn late_reply_log_fixture() {
    #[derive(Clone, Default)]
    struct Sink(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("lock").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let sink = Sink::default();
    let make_writer = {
        let sink = sink.clone();
        move || sink.clone()
    };
    let subscriber = tracing_subscriber::fmt()
        .with_writer(make_writer)
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let short = Duration::from_millis(40);
    let (transport, peer) = Transport::memory(16);
    let _daemon = fake_daemon(peer, move |frame, out| {
        let out = out.clone();
        Box::pin(async move {
            tokio::time::sleep(short * 3).await;
            let _ = out
                .send(ServerFrame::Reply {
                    id: frame.id,
                    result: Ok(Reply::Ok),
                })
                .await;
            true
        })
    });
    let client = Client::open(transport)
        .await
        .expect("open")
        .with_request_timeout(short);
    client
        .request(Command::Status)
        .await
        .expect_err("times out before the late reply arrives");
    // Give the reader loop time to receive the late reply and log it.
    tokio::time::sleep(short * 4).await;

    let logged = String::from_utf8(sink.0.lock().expect("lock").clone()).expect("utf8");
    assert!(
        logged.contains("status") && logged.contains("already gave up"),
        "the late reply must be logged by command name: {logged:?}"
    );
}

/// A version outside the served window is refused at hello in both
/// directions, so neither side ever gets far enough to send a command the
/// other cannot decode. Inside the window the daemon translates instead.
#[tokio::test]
async fn a_version_outside_the_window_is_refused_at_hello_in_either_direction() {
    for theirs in [
        aether_protocol::compat::MIN_PROTOCOL_VERSION - 1,
        PROTOCOL_VERSION + 1,
    ] {
        let hello = Hello {
            protocol_version: theirs,
            client_name: "aether".into(),
            client_version: "test".into(),
            features: vec![],
            token: None,
            device: None,
        };
        let err = aether_protocol::compat::negotiate(&hello)
            .expect_err("a daemon refuses a client built against another shape");
        assert_eq!(err.code, ErrorCode::Incompatible);

        // And a client refuses a daemon of another version at open.
        let (transport, mut peer) = Transport::memory(16);
        tokio::spawn(async move {
            if let Some(frame) = peer.incoming.recv().await {
                let _ = peer
                    .outgoing
                    .send(ServerFrame::Reply {
                        id: frame.id,
                        result: Err(ErrorReply::new(
                            ErrorCode::Incompatible,
                            format!("client protocol {PROTOCOL_VERSION} does not match daemon protocol {theirs}"),
                        )),
                    })
                    .await;
            }
        });
        let err = Client::open(transport).await.expect_err("refused");
        assert!(matches!(err, ClientError::Incompatible(_)), "{err:?}");
    }
}

/// Half of the client/daemon seam: the reply shape each of these methods
/// accepts. `aether-daemon`'s own dispatch test pins the other half - the
/// shape the daemon actually produces - and the two must name the same
/// variant or the command cannot work at all.
#[tokio::test]
async fn pause_resume_and_subtree_send_accept_the_daemon_reply_shapes() {
    let (transport, peer) = Transport::memory(16);
    let agent = AgentId::new();
    let _daemon = fake_daemon(peer, move |frame, out| {
        let out = out.clone();
        Box::pin(async move {
            let reply = match frame.command {
                Command::Pause { .. } => Reply::Paused {
                    agents: Vec::new(),
                    mid_turn: vec![agent],
                },
                Command::Resume { .. } => Reply::Agents(Vec::new()),
                Command::SendMessage { subtree: true, .. } => Reply::MessagesSent {
                    deliveries: Vec::new(),
                },
                other => panic!("unexpected command {other:?}"),
            };
            let _ = out
                .send(ServerFrame::Reply {
                    id: frame.id,
                    result: Ok(reply),
                })
                .await;
            true
        })
    });
    let client = Client::open(transport).await.expect("open");
    let (paused, mid_turn) = client.pause(agent, true).await.expect("pause");
    assert!(paused.is_empty());
    assert_eq!(mid_turn, vec![agent]);
    assert!(client.resume(agent, true).await.expect("resume").is_empty());
    assert!(
        client
            .send_message_subtree(agent, "stand down")
            .await
            .expect("fan out")
            .is_empty()
    );
}

#[tokio::test]
async fn github_watcher_methods_preserve_configuration_revision() {
    let (transport, peer) = Transport::memory(16);
    let _daemon = fake_daemon(peer, move |frame, out| {
        let out = out.clone();
        Box::pin(async move {
            let status = match frame.command {
                Command::GetGithubWatcher => aether_core::github::GithubWatcherStatus::default(),
                Command::ConfigureGithubWatcher {
                    config,
                    expected_revision,
                } => {
                    assert_eq!(expected_revision, 7);
                    assert!(config.enabled);
                    aether_core::github::GithubWatcherStatus {
                        revision: 8,
                        config,
                        repositories: Vec::new(),
                    }
                }
                other => panic!("unexpected command {other:?}"),
            };
            out.send(ServerFrame::Reply {
                id: frame.id,
                result: Ok(Reply::GithubWatcher(status)),
            })
            .await
            .expect("reply");
            true
        })
    });
    let client = Client::open(transport).await.expect("open");
    let mut config = client.github_watcher().await.expect("status").config;
    config.enabled = true;
    let updated = client
        .configure_github_watcher(config, 7)
        .await
        .expect("save");
    assert_eq!(updated.revision, 8);
}

#[tokio::test]
async fn concurrent_requests_are_routed_by_id() {
    let (transport, mut peer) = Transport::memory(16);
    tokio::spawn(async move {
        let hello = peer.incoming.recv().await.expect("hello");
        let _ = peer
            .outgoing
            .send(ServerFrame::Reply {
                id: hello.id,
                result: Ok(Reply::Welcome(welcome())),
            })
            .await;
        let first = peer.incoming.recv().await.expect("first");
        let second = peer.incoming.recv().await.expect("second");
        // Answer out of order; each reply's pid encodes its request id.
        for frame in [second, first] {
            let _ = peer
                .outgoing
                .send(ServerFrame::Reply {
                    id: frame.id,
                    result: Ok(Reply::Status(status(frame.id as u32))),
                })
                .await;
        }
    });
    let client = Client::open(transport).await.expect("open");
    let (a, b) = tokio::join!(client.status(), client.status());
    let a = a.expect("a");
    let b = b.expect("b");
    assert_ne!(a.pid, b.pid);
    // Request ids after the hello (id 1) are 2 and 3.
    assert_eq!(a.pid.min(b.pid), 2);
    assert_eq!(a.pid.max(b.pid), 3);
}

#[tokio::test]
async fn subscription_frames_reach_their_subscription() {
    let (transport, peer) = Transport::memory(16);
    let _daemon = fake_daemon(peer, |frame, out| {
        let out = out.clone();
        Box::pin(async move {
            if let Command::Subscribe { .. } = frame.command {
                let _ = out
                    .send(ServerFrame::Reply {
                        id: frame.id,
                        result: Ok(Reply::Subscribed { id: 7, last_seq: 3 }),
                    })
                    .await;
                let event = Event::from_draft(
                    EventDraft::new(Actor::System, EventKind::AgentReady),
                    EventId::new(),
                    4,
                    now(),
                );
                let _ = out
                    .send(ServerFrame::Event {
                        sub: 7,
                        event: event.clone(),
                    })
                    .await;
                let _ = out.send(ServerFrame::Event { sub: 8, event }).await;
                let _ = out
                    .send(ServerFrame::Delta {
                        sub: 7,
                        agent: AgentId::new(),
                        invocation: InvocationId::new(),
                        delta: Delta::Text { text: "hi".into() },
                    })
                    .await;
            }
            true
        })
    });
    let client = Client::open(transport).await.expect("open");
    let mut sub = client
        .subscribe(None, EventQuery::default(), true)
        .await
        .expect("subscribe");
    assert_eq!(sub.id(), 7);
    assert_eq!(sub.last_seq(), 3);
    let first = sub.next().await.expect("event");
    assert!(matches!(first, SubscriptionFrame::Event(e) if e.seq == 4));
    let second = sub.next().await.expect("delta");
    assert!(
        matches!(second, SubscriptionFrame::Delta { delta: Delta::Text { text }, .. } if text == "hi")
    );
}

#[tokio::test(start_paused = true)]
async fn a_late_subscription_reply_releases_the_daemon_subscription() {
    let (transport, mut peer) = Transport::memory(16);
    let client = Client::from_transport(transport).with_request_timeout(Duration::from_millis(20));
    let subscribing = tokio::spawn({
        let client = client.clone();
        async move { client.subscribe(None, EventQuery::default(), true).await }
    });
    let request = peer.incoming.recv().await.expect("subscribe request");
    assert!(matches!(
        subscribing.await.expect("task"),
        Err(ClientError::TimedOut { .. })
    ));
    peer.outgoing
        .send(ServerFrame::Reply {
            id: request.id,
            result: Ok(Reply::Subscribed { id: 7, last_seq: 3 }),
        })
        .await
        .expect("late reply");
    let cleanup = tokio::time::timeout(Duration::from_secs(1), peer.incoming.recv())
        .await
        .expect("abandoned subscription must be cancelled")
        .expect("unsubscribe request");
    assert!(matches!(cleanup.command, Command::Unsubscribe { id: 7 }));
    assert!(client.shared.subscriptions.lock().await.is_empty());
    assert!(client.shared.sub_receivers.lock().await.is_empty());
    assert!(client.shared.late_after_timeout.lock().await.is_empty());
    peer.outgoing
        .send(ServerFrame::Reply {
            id: cleanup.id,
            result: Ok(Reply::Ok),
        })
        .await
        .expect("unsubscribe reply");
}

#[tokio::test(start_paused = true)]
async fn a_subscription_timeout_during_registration_releases_both_channels() {
    let (transport, mut peer) = Transport::memory(16);
    let client = Client::from_transport(transport).with_request_timeout(Duration::from_millis(20));
    let registering = client.shared.subscriptions.lock().await;
    let subscribing = tokio::spawn({
        let client = client.clone();
        async move { client.subscribe(None, EventQuery::default(), true).await }
    });
    let request = peer.incoming.recv().await.expect("subscribe request");
    peer.outgoing
        .send(ServerFrame::Reply {
            id: request.id,
            result: Ok(Reply::Subscribed { id: 7, last_seq: 3 }),
        })
        .await
        .expect("reply");
    assert!(matches!(
        subscribing.await.expect("task"),
        Err(ClientError::TimedOut { .. })
    ));
    drop(registering);
    let cleanup = tokio::time::timeout(Duration::from_secs(1), peer.incoming.recv())
        .await
        .expect("subscription whose waiter expired must be cancelled")
        .expect("unsubscribe request");
    assert!(matches!(cleanup.command, Command::Unsubscribe { id: 7 }));
    assert!(client.shared.subscriptions.lock().await.is_empty());
    assert!(client.shared.sub_receivers.lock().await.is_empty());
    assert!(client.shared.late_after_timeout.lock().await.is_empty());
    peer.outgoing
        .send(ServerFrame::Reply {
            id: cleanup.id,
            result: Ok(Reply::Ok),
        })
        .await
        .expect("unsubscribe reply");
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_subscription_waiter_releases_the_daemon_subscription() {
    let (transport, mut peer) = Transport::memory(16);
    let client = Client::from_transport(transport);
    let subscribing = tokio::spawn({
        let client = client.clone();
        async move { client.subscribe(None, EventQuery::default(), true).await }
    });
    let request = peer.incoming.recv().await.expect("subscribe request");
    subscribing.abort();
    assert!(subscribing.await.expect_err("cancelled").is_cancelled());
    peer.outgoing
        .send(ServerFrame::Reply {
            id: request.id,
            result: Ok(Reply::Subscribed { id: 7, last_seq: 3 }),
        })
        .await
        .expect("reply");
    let cleanup = tokio::time::timeout(Duration::from_secs(1), peer.incoming.recv())
        .await
        .expect("cancelled waiter must release the subscription")
        .expect("unsubscribe request");
    assert!(matches!(cleanup.command, Command::Unsubscribe { id: 7 }));
    assert!(client.shared.subscriptions.lock().await.is_empty());
    assert!(client.shared.sub_receivers.lock().await.is_empty());
    assert!(client.shared.late_after_timeout.lock().await.is_empty());
    peer.outgoing
        .send(ServerFrame::Reply {
            id: cleanup.id,
            result: Ok(Reply::Ok),
        })
        .await
        .expect("unsubscribe reply");
}

#[tokio::test(start_paused = true)]
async fn cancelling_a_queued_subscription_reply_releases_its_channels() {
    use std::future::Future;
    use std::task::{Context, Waker};

    for cancel_during_claim in [false, true] {
        let (transport, mut peer) = Transport::memory(16);
        let client = Client::from_transport(transport);
        let mut subscribing = Box::pin(client.subscribe(None, EventQuery::default(), true));
        assert!(
            subscribing
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        let request = peer.incoming.recv().await.expect("subscribe request");
        peer.outgoing
            .send(ServerFrame::Reply {
                id: request.id,
                result: Ok(Reply::Subscribed { id: 7, last_seq: 3 }),
            })
            .await
            .expect("subscribe reply");

        // A subsequent reply proves the reader queued the subscription reply.
        let ping = tokio::spawn({
            let client = client.clone();
            async move { client.request(Command::Ping).await }
        });
        let request = peer.incoming.recv().await.expect("ping request");
        assert!(matches!(request.command, Command::Ping));
        peer.outgoing
            .send(ServerFrame::Reply {
                id: request.id,
                result: Ok(Reply::Ok),
            })
            .await
            .expect("ping reply");
        ping.await.expect("ping task").expect("ping");

        if cancel_during_claim {
            let claiming = client.shared.sub_receivers.lock().await;
            assert!(
                subscribing
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending()
            );
            drop(subscribing);
            drop(claiming);
        } else {
            drop(subscribing);
        }
        let cleanup = tokio::time::timeout(Duration::from_secs(1), peer.incoming.recv())
            .await
            .expect("cancelled subscriber must release its queued subscription")
            .expect("unsubscribe request");
        assert!(matches!(cleanup.command, Command::Unsubscribe { id: 7 }));
        assert!(client.shared.subscriptions.lock().await.is_empty());
        assert!(client.shared.sub_receivers.lock().await.is_empty());
        peer.outgoing
            .send(ServerFrame::Reply {
                id: cleanup.id,
                result: Ok(Reply::Ok),
            })
            .await
            .expect("unsubscribe reply");
    }
}

#[tokio::test(start_paused = true)]
async fn abandoned_subscription_cleanup_cannot_hold_a_stalled_sink_forever() {
    let (transport, mut peer) = Transport::memory(1);
    let client = Client::from_transport(transport).with_request_timeout(Duration::from_millis(20));
    let subscribing = tokio::spawn({
        let client = client.clone();
        async move { client.subscribe(None, EventQuery::default(), true).await }
    });
    let request = peer.incoming.recv().await.expect("subscribe request");
    assert!(matches!(
        subscribing.await.expect("task"),
        Err(ClientError::TimedOut { .. })
    ));
    client
        .sink
        .send(ClientFrame {
            id: 999,
            command: Command::Ping,
        })
        .await
        .expect("fill outgoing queue");
    peer.outgoing
        .send(ServerFrame::Reply {
            id: request.id,
            result: Ok(Reply::Subscribed { id: 7, last_seq: 3 }),
        })
        .await
        .expect("late reply");
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    let sink = client.sink.downgrade();
    tokio::time::sleep(DEFAULT_REQUEST_TIMEOUT * 2).await;
    assert!(client.shared.pending.lock().await.is_empty());
    assert!(client.shared.subscriptions.lock().await.is_empty());
    assert!(client.shared.sub_receivers.lock().await.is_empty());
    drop(client);
    assert!(
        sink.upgrade().is_none(),
        "cleanup must release the blocked sink"
    );
}

#[tokio::test]
async fn disconnect_fails_pending_and_future_requests() {
    let (transport, mut peer) = Transport::memory(16);
    let client_task = tokio::spawn(async move {
        let hello = peer.incoming.recv().await.expect("hello");
        let _ = peer
            .outgoing
            .send(ServerFrame::Reply {
                id: hello.id,
                result: Ok(Reply::Welcome(welcome())),
            })
            .await;
        // Receive one request, then drop the connection without answering.
        let _ = peer.incoming.recv().await;
        drop(peer);
    });
    let client = Client::open(transport).await.expect("open");
    let err = client.status().await.unwrap_err();
    assert!(matches!(err, ClientError::Disconnected));
    client_task.await.expect("task");
    let err = client.status().await.unwrap_err();
    assert!(matches!(err, ClientError::Disconnected));
    assert!(!client.is_connected());
}

#[tokio::test]
async fn connection_diagnostic_fails_every_pending_request_with_the_reason() {
    let (transport, mut peer) = Transport::memory(16);
    let client = Client::from_transport(transport).without_request_timeout();
    let reason = ErrorReply::invalid("bad frame: invalid `req`: missing field `req`");
    let diagnostic = reason.clone();
    let daemon = tokio::spawn(async move {
        peer.incoming.recv().await.expect("first request");
        peer.incoming.recv().await.expect("second request");
        peer.outgoing
            .send(ServerFrame::Reply {
                id: 0,
                result: Err(diagnostic),
            })
            .await
            .unwrap();
        // EOF settles both requests; the notice alone is not a reply to either.
    });
    let (first, second) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(
            client.request(Command::Ping),
            client.request(Command::Status)
        )
    })
    .await
    .expect("closure must wake both requests without a request timeout");
    for result in [first, second] {
        assert!(matches!(result, Err(ClientError::Protocol(ref error)) if error == &reason));
    }
    assert_eq!(client.disconnect_reason(), Some(reason));
    assert!(!client.is_connected());
    assert!(matches!(
        client.request(Command::Ping).await,
        Err(ClientError::Disconnected)
    ));
    daemon.await.unwrap();
}

#[tokio::test]
async fn request_waiting_for_the_pending_lock_cannot_register_after_close_drain() {
    let (transport, _peer) = Transport::memory(16);
    let client = Client::from_transport(transport);
    let mut pending = client.shared.pending.lock().await;
    let before = client.shared.next_id.load(Ordering::SeqCst);
    let request = tokio::spawn({
        let client = client.clone();
        async move { client.request(Command::Ping).await }
    });
    while client.shared.next_id.load(Ordering::SeqCst) == before {
        tokio::task::yield_now().await;
    }
    client.shared.closed.store(true, Ordering::SeqCst);
    pending.clear();
    drop(pending);
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), request)
            .await
            .unwrap()
            .unwrap(),
        Err(ClientError::Disconnected)
    ));
    assert!(client.shared.pending.lock().await.is_empty());
}

#[tokio::test]
async fn dropping_a_claimed_subscription_outside_the_runtime_unsubscribes() {
    let (transport, mut peer) = Transport::memory(16);
    let client = Client::from_transport(transport);
    let subscribing = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .subscribe(None, EventQuery::default(), false)
                .await
                .unwrap()
        }
    });
    let request = peer.incoming.recv().await.unwrap();
    peer.outgoing
        .send(ServerFrame::Reply {
            id: request.id,
            result: Ok(Reply::Subscribed { id: 7, last_seq: 0 }),
        })
        .await
        .unwrap();
    let subscription = subscribing.await.unwrap();
    std::thread::spawn(move || drop(subscription))
        .join()
        .unwrap();
    let cleanup = tokio::time::timeout(Duration::from_secs(1), peer.incoming.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(cleanup.command, Command::Unsubscribe { id: 7 }));
    assert!(client.shared.subscriptions.lock().await.is_empty());
    assert!(client.shared.sub_receivers.lock().await.is_empty());
    peer.outgoing
        .send(ServerFrame::Reply {
            id: cleanup.id,
            result: Ok(Reply::Ok),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn memory_proposals_are_refused_without_the_feature_before_sending() {
    let (transport, peer) = Transport::memory(16);
    let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = sent.clone();
    let _daemon = fake_daemon(peer, move |_, _| {
        seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { true })
    });
    let client = Client::open(transport).await.unwrap();
    let spec = serde_json::from_value(
        serde_json::json!({"text":"A durable lesson", "kind":"lesson",
        "scope":{"scope":"global"},"proposed":true}),
    )
    .unwrap();
    assert!(matches!(
        client.create_memory(spec).await,
        Err(ClientError::Incompatible(_))
    ));
    let id = aether_core::id::MemoryId::new();
    assert!(matches!(
        client.accept_memory(id, None).await,
        Err(ClientError::Incompatible(_))
    ));
    assert!(matches!(
        client.reject_memory(id, "No evidence".into()).await,
        Err(ClientError::Incompatible(_))
    ));
    let query = serde_json::from_value(serde_json::json!({"proposed":true})).unwrap();
    assert!(matches!(
        client.memories(query).await,
        Err(ClientError::Incompatible(_))
    ));
    assert_eq!(sent.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn memories_in_context_is_refused_without_the_feature_before_sending() {
    let (transport, peer) = Transport::memory(16);
    let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = sent.clone();
    let _daemon = fake_daemon(peer, move |_, _| {
        seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { true })
    });
    let client = Client::open(transport).await.unwrap();
    let task = Some(aether_core::id::TaskId::new());
    assert!(matches!(
        client.memories_in_context(task, None).await,
        Err(ClientError::Incompatible(_))
    ));
    assert_eq!(sent.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn memory_feedback_is_refused_without_the_feature_before_sending() {
    use aether_core::id::{MemoryFeedbackId, MemoryId, TaskId};
    let (transport, peer) = Transport::memory(16);
    let sent = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = sent.clone();
    let _daemon = fake_daemon(peer, move |_, _| {
        seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { true })
    });
    let client = Client::open(transport).await.unwrap();
    let memory = MemoryId::new();
    let spec = serde_json::from_value(serde_json::json!({
        "memory":memory, "task":TaskId::new(), "verdict":"neutral",
        "evidence":"operator", "reason":"No effect observed",
    }))
    .unwrap();
    assert!(matches!(
        client.record_memory_feedback(spec).await,
        Err(ClientError::Incompatible(_))
    ));
    assert!(matches!(
        client
            .retract_memory_feedback(MemoryFeedbackId::new(), "Withdraw".into())
            .await,
        Err(ClientError::Incompatible(_))
    ));
    assert!(matches!(
        client.memory_feedback(memory, None).await,
        Err(ClientError::Incompatible(_))
    ));
    assert_eq!(sent.load(std::sync::atomic::Ordering::SeqCst), 0);
}
