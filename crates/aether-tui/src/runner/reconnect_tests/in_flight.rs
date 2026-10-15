use super::*;
use aether_protocol::{Reply, ServerFrame};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;

#[derive(Clone, Copy)]
enum Delayed {
    Tree,
    Status,
}

impl Delayed {
    fn matches(self, frame: &ServerFrame) -> bool {
        matches!(
            (self, frame),
            (
                Self::Tree,
                ServerFrame::Reply {
                    result: Ok(Reply::Tree(_)),
                    ..
                }
            ) | (
                Self::Status,
                ServerFrame::Reply {
                    result: Ok(Reply::Status(_)),
                    ..
                }
            )
        )
    }
}

struct Gate {
    client: Client,
    ready: oneshot::Receiver<()>,
    release: oneshot::Sender<()>,
    proxy: tokio::task::JoinHandle<()>,
    _directory: tempfile::TempDir,
}

async fn delayed_client(socket: std::path::PathBuf, delayed: Delayed) -> Gate {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("p.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let (ready_tx, ready) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    let proxy = tokio::spawn(async move {
        let (client, _) = listener.accept().await.unwrap();
        let daemon = UnixStream::connect(socket).await.unwrap();
        let (mut client_read, client_write) = client.into_split();
        let (daemon_read, mut daemon_write) = daemon.into_split();
        let uplink = tokio::io::copy(&mut client_read, &mut daemon_write);
        let downlink = delay_response(daemon_read, client_write, delayed, ready_tx, release_rx);
        tokio::select! {
            result = uplink => { result.unwrap(); }
            () = downlink => {}
        }
    });
    let client = Client::connect(&path).await.unwrap();
    assert!(client.is_connected());
    assert!(client.welcome().is_some());
    Gate {
        client,
        ready,
        release,
        proxy,
        _directory: directory,
    }
}

async fn delay_response(
    read: tokio::net::unix::OwnedReadHalf,
    mut write: tokio::net::unix::OwnedWriteHalf,
    delayed: Delayed,
    ready: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
) {
    let mut gate = Some((ready, release));
    let mut lines = BufReader::new(read).lines();
    while let Some(line) = lines.next_line().await.unwrap() {
        let frame: ServerFrame = aether_protocol::decode(&line).unwrap();
        if delayed.matches(&frame)
            && let Some((ready, release)) = gate.take()
        {
            assert!(matches!(frame, ServerFrame::Reply { id, .. } if id > 0));
            ready.send(()).unwrap();
            release.await.unwrap();
        }
        write.write_all(line.as_bytes()).await.unwrap();
        write.write_all(b"\n").await.unwrap();
    }
    assert!(gate.is_none(), "connection ended before its gated response");
}

async fn old_reply(rx: &mut mpsc::UnboundedReceiver<LoopEvent>, app: &mut App) {
    let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let LoopEvent::AppFrom { instance, event } = event else {
        panic!("scoped old reply");
    };
    assert_eq!(
        Some(instance),
        app.options.instance_id,
        "runner's instance filter accepts the reply"
    );
    assert!(matches!(event, AppEvent::Reply { result: Ok(_), .. }));
    let effects = app.reduce(event);
    assert!(
        !effects.iter().any(|effect| matches!(
            effect,
            Effect::Request {
                command: Command::GetTree { .. } | Command::Status,
                ..
            }
        )),
        "the stale reply does not schedule a corrective snapshot"
    );
}

#[tokio::test]
#[ignore = "reproduces #16 reconnect mismatch: old connection tree reply restores stale agent status"]
async fn reconnect_late_old_tree_reply_must_not_restore_pre_outage_status() {
    let mut fixture = Fixture::new().await;
    let mut live = fixture.fresh().await;
    let gate = delayed_client(fixture.socket(), Delayed::Tree).await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    spawn_request(
        gate.client.clone(),
        tx,
        Command::GetTree {
            run: None,
            include_archived: false,
        },
        Then::Tree,
    );
    tokio::time::timeout(Duration::from_secs(5), gate.ready)
        .await
        .unwrap()
        .unwrap();
    live.reduce(AppEvent::Frame(SubscriptionFrame::Lagged { capacity: 1 }));
    fixture.agent.status = AgentStatus::Failed {
        error: "failed during outage".into(),
    };
    fixture.put_agent(fixture.agent.clone());
    fixture.commit(vec![EventKind::AgentFailed {
        error: "failed during outage".into(),
    }]);
    fixture.reconnect(&mut live).await;
    let fresh = fixture.fresh().await;
    assert_eq!(
        live.tree, fresh.tree,
        "reconnected snapshot is initially correct"
    );
    gate.release.send(()).unwrap();
    old_reply(&mut rx, &mut live).await;
    fixture.stop().await;
    gate.proxy.await.unwrap();
    assert_eq!(
        live.selected_agent().unwrap().status,
        fresh.selected_agent().unwrap().status
    );
}

#[tokio::test]
#[ignore = "reproduces #16 reconnect mismatch: old connection status reply rolls back usage totals"]
async fn reconnect_late_old_status_reply_must_not_roll_back_usage_totals() {
    let mut fixture = Fixture::new().await;
    let mut live = fixture.fresh().await;
    let gate = delayed_client(fixture.socket(), Delayed::Status).await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    spawn_request(gate.client.clone(), tx, Command::Status, Then::StatusPoll);
    tokio::time::timeout(Duration::from_secs(5), gate.ready)
        .await
        .unwrap()
        .unwrap();
    live.reduce(AppEvent::Frame(SubscriptionFrame::Lagged { capacity: 1 }));
    let usage = Usage {
        input_tokens: 42_000,
        cost_usd: 1.25,
        ..Default::default()
    };
    fixture.agent.stats.usage = usage;
    fixture.put_agent(fixture.agent.clone());
    fixture.commit(vec![EventKind::AgentUsageUpdated {
        total: usage,
        turn_usage: usage,
    }]);
    fixture.reconnect(&mut live).await;
    let fresh = fixture.fresh().await;
    assert_eq!(
        live.status.as_ref().unwrap().usage_total,
        fresh.status.as_ref().unwrap().usage_total
    );
    gate.release.send(()).unwrap();
    old_reply(&mut rx, &mut live).await;
    fixture.stop().await;
    gate.proxy.await.unwrap();
    assert_eq!(
        live.status.as_ref().unwrap().usage_total.input_tokens,
        fresh.status.as_ref().unwrap().usage_total.input_tokens
    );
}
