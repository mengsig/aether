use super::*;
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn shutdown_releases_a_connection_with_a_blocked_writer_and_full_reply_queue() {
    assert_stalled_connection_stops(true).await;
}

#[tokio::test]
async fn shutdown_releases_a_connection_with_only_its_writer_blocked() {
    assert_stalled_connection_stops(false).await;
}

async fn assert_stalled_connection_stops(fill_queue: bool) {
    let (daemon, _config) = daemon().await;
    let daemon = Arc::new(daemon);
    let runtime = daemon.runtime.clone();
    let weak = Arc::downgrade(&daemon);
    let handshaking = Arc::new(tokio::sync::Semaphore::new(1));
    let authenticated = Arc::new(tokio::sync::Semaphore::new(1));
    let gate = Gate::new(
        handshaking.clone().acquire_owned().await.unwrap(),
        authenticated.clone(),
    );
    let shutdown = CancellationToken::new();
    let (mut client, server) = tokio::io::duplex(128);
    let mut serving = tokio::spawn(serve(
        daemon,
        server,
        ConnectionPolicy::default(),
        None,
        Some(gate),
        shutdown.clone(),
    ));
    let hello = aether_protocol::encode(&ClientFrame {
        id: 1,
        command: Command::Hello(aether_protocol::Hello::new("shutdown-test", "test")),
    })
    .unwrap();
    client.write_all(hello.as_bytes()).await.unwrap();
    // The client never reads, so even the welcome blocks the writer.
    let ping = aether_protocol::encode(&ClientFrame {
        id: 2,
        command: Command::Ping,
    })
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while authenticated.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the peer is admitted");
    if fill_queue {
        let filled = tokio::time::timeout(Duration::from_millis(200), async {
            // Past the reply queue and the reads that may wait to run.
            for _ in 0..1100 + crate::connection::lanes::MAX_QUEUED_READS {
                client.write_all(ping.as_bytes()).await.unwrap();
            }
        })
        .await;
        assert!(filled.is_err(), "the client must reach reply back-pressure");
    }
    shutdown.cancel();
    let stopped = tokio::time::timeout(Duration::from_secs(3), &mut serving).await;
    if stopped.is_err() {
        serving.abort();
        let _ = serving.await;
    }
    runtime.shutdown().await;
    stopped
        .expect("shutdown must release a stalled connection")
        .unwrap()
        .unwrap();
    assert_eq!(authenticated.available_permits(), 1);
    assert_eq!(handshaking.available_permits(), 1);
    assert!(weak.upgrade().is_none(), "connection dropped its daemon");
}

#[tokio::test]
async fn shutdown_closes_the_tcp_listener_and_its_accepted_connections() {
    let (daemon, _config) = daemon().await;
    let daemon = Arc::new(daemon);
    let runtime = daemon.runtime.clone();
    let weak = Arc::downgrade(&daemon);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(crate::serve_tcp(
        daemon,
        listener,
        crate::TcpListen::new(address, "0123456789abcdef0123456789abcdef"),
        shutdown.clone(),
    ));
    let mut peer = tokio::net::TcpStream::connect(address).await.unwrap();
    let hello = aether_protocol::encode(&ClientFrame {
        id: 1,
        command: Command::Hello(
            aether_protocol::Hello::new("shutdown-test", "test")
                .with_token("0123456789abcdef0123456789abcdef"),
        ),
    })
    .unwrap();
    peer.write_all(hello.as_bytes()).await.unwrap();
    let mut input = FramedRead::new(peer, LinesCodec::new());
    let welcome = tokio::time::timeout(Duration::from_secs(3), input.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        aether_protocol::decode::<ServerFrame>(&welcome).unwrap(),
        ServerFrame::Reply {
            result: Ok(Reply::Welcome(_)),
            ..
        }
    ));
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    assert!(
        weak.upgrade().is_none(),
        "listener and connections dropped the daemon"
    );
    assert!(input.next().await.is_none(), "connection closed");
    // A child that a parallel test forks holds a copy of the listener until it
    // execs, so the port can stay taken briefly; a leaked listener never lets go.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while let Err(error) = tokio::net::TcpListener::bind(address).await {
        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse, "{error}");
        assert!(
            tokio::time::Instant::now() < deadline,
            "port still held: {error}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    runtime.shutdown().await;
}

#[test]
fn shutdown_finishes_an_inflight_artifact_publication() {
    use aether_store::Store as _;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let store =
            Arc::new(aether_store::SqliteStore::open(directory.path().join("store.db")).unwrap());
        let (daemon, _config) = daemon_with_store(store.clone()).await;
        let daemon = Arc::new(daemon);
        let (_, task) = daemon
            .runtime
            .create_agent(
                CreateAgentSpec {
                    role: "worker".into(),
                    objective: Some("publish evidence".into()),
                    // No workspace: keeps the test independent of the host checkout.
                    scope: Some(Default::default()),
                    start: false,
                    ..Default::default()
                },
                Actor::user(),
            )
            .unwrap();
        let task = task.unwrap();
        let shutdown = CancellationToken::new();
        let (client, server) = tokio::io::duplex(16 * 1024);
        let (read, mut write) = tokio::io::split(client);
        let mut input = FramedRead::new(read, LinesCodec::new());
        let serving = serve(
            daemon.clone(),
            server,
            ConnectionPolicy::default(),
            None,
            None,
            shutdown.clone(),
        );
        tokio::pin!(serving);
        let hello = aether_protocol::encode(&ClientFrame {
            id: 1,
            command: Command::Hello(aether_protocol::Hello::new("shutdown-test", "test")),
        })
        .unwrap();
        write.write_all(hello.as_bytes()).await.unwrap();
        tokio::select! {
            reply = input.next() => {
                let frame: ServerFrame = aether_protocol::decode(&reply.unwrap().unwrap()).unwrap();
                assert!(matches!(frame, ServerFrame::Reply { result: Ok(Reply::Welcome(_)), .. }));
            },
            _ = &mut serving => panic!("connection ended before greeting"),
        }
        let (release, blocked) = std::sync::mpsc::channel();
        let (started, ready) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            // A dropped release sender also unblocks cleanup after a failed assertion.
            let _ = blocked.recv();
        });
        ready.await.unwrap();
        let publication = aether_protocol::encode(&ClientFrame {
            id: 2,
            command: Command::PublishArtifact(aether_protocol::PublishArtifact {
                task: task.id,
                name: "evidence".into(),
                summary: "completed during shutdown".into(),
                content_type: "text/plain".into(),
                text: "saved evidence".into(),
            }),
        })
        .unwrap();
        write.write_all(publication.as_bytes()).await.unwrap();
        // Poll through dispatch until its artifact write queues behind the blocker.
        assert!(futures::poll!(&mut serving).is_pending());
        shutdown.cancel();
        let finishing = futures::poll!(&mut serving).is_pending();
        release.send(()).unwrap();
        blocker.await.unwrap();
        if finishing {
            // Durable writes can take seconds on a busy filesystem; this bounds
            // liveness, rather than requiring a particular storage latency.
            tokio::time::timeout(Duration::from_secs(30), &mut serving)
                .await
                .expect("shutdown must finish the accepted artifact publication")
                .unwrap();
        }
        daemon.runtime.shutdown().await;
        assert!(finishing, "shutdown must wait for the accepted mutation");
        let artifacts = store.live_artifacts().unwrap();
        assert_eq!(artifacts.len(), 1, "the artifact write must have metadata");
        assert_eq!(
            store.read_artifact_bytes(artifacts[0].id).unwrap().unwrap(),
            b"saved evidence"
        );
    });
}

/// A client that sends `shutdown` behind replies it never reads still shuts
/// the daemon down, once the earlier commands have had their patience.
#[tokio::test(start_paused = true)]
async fn shutdown_behind_replies_nobody_reads_still_shuts_down() {
    let (daemon, _config) = daemon().await;
    let daemon = Arc::new(daemon);
    let runtime = daemon.runtime.clone();
    let shutdown = CancellationToken::new();
    // Frames go in freely; replies stop at 128 bytes, as no one reads them.
    let (mut requests, inbound) = tokio::io::duplex(1 << 20);
    let (_unread, outbound) = tokio::io::duplex(128);
    let (inbound, _) = tokio::io::split(inbound);
    let (_, outbound) = tokio::io::split(outbound);
    let serving = tokio::spawn(serve(
        daemon,
        tokio::io::join(inbound, outbound),
        ConnectionPolicy::default(),
        None,
        None,
        shutdown.clone(),
    ));
    let mut frames = aether_protocol::encode(&ClientFrame {
        id: 1,
        command: Command::Hello(aether_protocol::Hello::new("shutdown-test", "test")),
    })
    .unwrap();
    // Past the reply queue, short of the reads that stop the connection reading.
    let pings = 1024 + crate::connection::lanes::MAX_QUEUED_READS / 2;
    for id in 2..2 + pings as u64 {
        let ping = ClientFrame {
            id,
            command: Command::Ping,
        };
        frames.push_str(&aether_protocol::encode(&ping).unwrap());
    }
    let stop = ClientFrame {
        id: u64::MAX,
        command: Command::Shutdown,
    };
    frames.push_str(&aether_protocol::encode(&stop).unwrap());
    requests.write_all(frames.as_bytes()).await.unwrap();

    let stopped = tokio::time::timeout(3 * crate::connection::lanes::DRAIN_PATIENCE, serving).await;
    assert!(
        shutdown.is_cancelled(),
        "shutdown was not held off for good"
    );
    stopped
        .expect("the connection closes once shutdown gave up on its replies")
        .unwrap()
        .unwrap();
    runtime.shutdown().await;
}

/// Patience gives up unread replies, never a command still running: one slower
/// than it finishes before `shutdown` answers, and its reply still goes out.
#[test]
fn finishing_waits_past_its_patience_for_a_running_command() {
    use aether_store::Store as _;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let store =
            Arc::new(aether_store::SqliteStore::open(directory.path().join("store.db")).unwrap());
        let (daemon, _config) = daemon_with_store(store.clone()).await;
        let daemon = Arc::new(daemon);
        let (_, task) = daemon
            .runtime
            .create_agent(
                CreateAgentSpec {
                    role: "worker".into(),
                    objective: Some("publish evidence".into()),
                    scope: Some(Default::default()),
                    start: false,
                    ..Default::default()
                },
                Actor::user(),
            )
            .unwrap();
        let task = task.unwrap();
        let (out, mut replies) = tokio::sync::mpsc::channel(8);
        let mut lanes =
            crate::connection::lanes::Lanes::new(daemon.clone(), out, CancellationToken::new());
        let (release, blocked) = std::sync::mpsc::channel();
        let (started, ready) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started.send(()).unwrap();
            let _ = blocked.recv();
        });
        ready.await.unwrap();
        let publication = Command::PublishArtifact(aether_protocol::PublishArtifact {
            task: task.id,
            name: "evidence".into(),
            summary: "published past the patience".into(),
            content_type: "text/plain".into(),
            text: "saved evidence".into(),
        });
        lanes
            .run(
                2,
                publication,
                &Actor::user(),
                &crate::authz::AuthContext::Local,
            )
            .await;

        let finishing = lanes.finish(Duration::from_millis(50));
        tokio::pin!(finishing);
        let early = tokio::time::timeout(Duration::from_millis(300), &mut finishing).await;
        release.send(()).unwrap();
        blocker.await.unwrap();
        assert!(
            early.is_err(),
            "finishing must wait for the running publication"
        );
        let answered = tokio::time::timeout(Duration::from_secs(30), finishing)
            .await
            .expect("the publication finishes once the blocking pool is free");
        assert!(!answered, "the publication outlasted the patience");
        let reply = replies.try_recv().expect("a reply with room is still sent");
        assert!(matches!(
            reply,
            ServerFrame::Reply {
                id: 2,
                result: Ok(_)
            }
        ));
        assert_eq!(store.live_artifacts().unwrap().len(), 1);
        daemon.runtime.shutdown().await;
    });
}
