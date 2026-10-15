use super::*;
use aether_protocol::{Reply, ServerFrame};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;

#[derive(Clone, Copy)]
enum Delayed {
    Tree,
    Status,
    /// Whichever of the two comes first; the other queues behind it.
    Snapshot,
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
            ) | (
                Self::Snapshot,
                ServerFrame::Reply {
                    result: Ok(Reply::Tree(_) | Reply::Status(_)),
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

async fn next_scoped(rx: &mut mpsc::UnboundedReceiver<LoopEvent>) -> (String, u64, AppEvent) {
    let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let LoopEvent::AppFrom {
        instance,
        generation,
        event,
    } = event
    else {
        panic!("scoped event");
    };
    (instance, generation, event)
}

/// Deliver the old connection's reply the way the loop does, through `current`.
async fn old_reply(rx: &mut mpsc::UnboundedReceiver<LoopEvent>, app: &mut App, current: &Link) {
    let (instance, generation, event) = next_scoped(rx).await;
    assert_eq!(
        Some(&instance),
        app.options.instance_id.as_ref(),
        "same daemon identity as the reconnected view"
    );
    assert!(matches!(event, AppEvent::Reply { result: Ok(_), .. }));
    let Some(event) = current.admit(&instance, generation, event) else {
        return;
    };
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
async fn reconnect_late_old_tree_reply_must_not_restore_pre_outage_status() {
    let mut fixture = Fixture::new().await;
    let mut live = fixture.fresh().await;
    let gate = delayed_client(fixture.socket(), Delayed::Tree).await;
    let old = Link::new(gate.client.clone());
    let (tx, mut rx) = mpsc::unbounded_channel();
    spawn_request(
        old.clone(),
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
    let current = fixture.reconnect_after(&mut live, &old).await;
    let fresh = fixture.fresh().await;
    assert_eq!(
        live.tree, fresh.tree,
        "reconnected snapshot is initially correct"
    );
    gate.release.send(()).unwrap();
    old_reply(&mut rx, &mut live, &current).await;
    fixture.stop().await;
    gate.proxy.await.unwrap();
    assert_eq!(
        live.selected_agent().unwrap().status,
        fresh.selected_agent().unwrap().status
    );
}

#[tokio::test]
async fn reconnect_late_old_status_reply_must_not_roll_back_usage_totals() {
    let mut fixture = Fixture::new().await;
    let mut live = fixture.fresh().await;
    let gate = delayed_client(fixture.socket(), Delayed::Status).await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let old = Link::new(gate.client.clone());
    spawn_request(old.clone(), tx, Command::Status, Then::StatusPoll);
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
    let current = fixture.reconnect_after(&mut live, &old).await;
    let fresh = fixture.fresh().await;
    assert_eq!(
        live.status.as_ref().unwrap().usage_total,
        fresh.status.as_ref().unwrap().usage_total
    );
    gate.release.send(()).unwrap();
    old_reply(&mut rx, &mut live, &current).await;
    fixture.stop().await;
    gate.proxy.await.unwrap();
    assert_eq!(
        live.status.as_ref().unwrap().usage_total.input_tokens,
        fresh.status.as_ref().unwrap().usage_total.input_tokens
    );
}

/// Far past the status refresh interval, so the tick is due to poll.
const POLL_FRAMES: u64 = 10_000;

fn take_request(effects: Vec<Effect>, wanted: &Then) -> Effect {
    let mut matching: Vec<_> = effects
        .into_iter()
        .filter(|effect| matches!(effect, Effect::Request { then, .. } if then == wanted))
        .collect();
    assert_eq!(matching.len(), 1, "exactly one {wanted:?} request");
    matching.pop().unwrap()
}

fn send_on(link: &Link, tx: &mpsc::UnboundedSender<LoopEvent>, effect: Effect) {
    let Effect::Request { command, then } = effect else {
        panic!("a request effect");
    };
    spawn_request(link.clone(), tx.clone(), command, then);
}

/// An agent event as the subscription delivers it, which asks for a tree refresh.
fn agent_event(fixture: &Fixture, app: &mut App) -> Vec<Effect> {
    let mut events = fixture.commit(vec![EventKind::AgentStarted { turn: 1 }]);
    let event = Box::new(events.pop().unwrap());
    app.reduce(AppEvent::Frame(SubscriptionFrame::Event(event)))
}

#[tokio::test]
async fn reconnect_dropped_old_snapshot_replies_leave_polling_and_tree_refresh_working() {
    let mut fixture = Fixture::new().await;
    let mut live = fixture.fresh().await;
    let gate = delayed_client(fixture.socket(), Delayed::Snapshot).await;
    let old = Link::new(gate.client.clone());
    let (tx, mut rx) = mpsc::unbounded_channel();
    let tick = AppEvent::Tick {
        frames: POLL_FRAMES,
    };
    send_on(
        &old,
        &tx,
        take_request(live.reduce(tick), &Then::StatusPoll),
    );
    send_on(
        &old,
        &tx,
        take_request(agent_event(&fixture, &mut live), &Then::Tree),
    );
    tokio::time::timeout(Duration::from_secs(5), gate.ready)
        .await
        .unwrap()
        .unwrap();
    live.reduce(AppEvent::Frame(SubscriptionFrame::Lagged { capacity: 1 }));
    let current = fixture.reconnect_after(&mut live, &old).await;
    gate.release.send(()).unwrap();
    for _ in 0..2 {
        let (instance, generation, event) = next_scoped(&mut rx).await;
        assert!(current.admit(&instance, generation, event).is_none());
    }
    let tick = AppEvent::Tick {
        frames: POLL_FRAMES,
    };
    let poll = take_request(live.reduce(tick), &Then::StatusPoll);
    fixture.answer(&mut live, &current.client, vec![poll]).await;
    let refresh = take_request(agent_event(&fixture, &mut live), &Then::Tree);
    fixture
        .answer(&mut live, &current.client, vec![refresh])
        .await;
    let fresh = fixture.fresh().await;
    fixture.stop().await;
    gate.proxy.await.unwrap();
    assert_eq!(
        live.status.as_ref().unwrap().usage_total,
        fresh.status.as_ref().unwrap().usage_total
    );
    assert_eq!(live.tree, fresh.tree);
}

/// Frames the old subscription delivered but the loop reaches only after the
/// reconnect are replayed by the new one; applying both would repeat them.
#[tokio::test]
async fn reconnect_old_subscription_frames_are_not_applied_twice() {
    let mut fixture = Fixture::new().await;
    let mut live = fixture.fresh().await;
    let cursor = live.last_seq;
    live.reduce(AppEvent::Frame(SubscriptionFrame::Lagged { capacity: 1 }));
    fixture.commit(vec![message("after lag")]);
    let old = Link::new(fixture.client().await);
    let (tx, mut rx) = mpsc::unbounded_channel();
    spawn_subscription(old.clone(), tx, cursor);
    let current = fixture.reconnect_after(&mut live, &old).await;
    let (_, _, subscribed) = next_scoped(&mut rx).await;
    assert!(matches!(subscribed, AppEvent::Subscribed { .. }));
    let (instance, generation, frame) = next_scoped(&mut rx).await;
    assert!(matches!(
        frame,
        AppEvent::Frame(SubscriptionFrame::Event(_))
    ));
    for event in [subscribed, frame] {
        if let Some(event) = current.admit(&instance, generation, event) {
            let effects = live.reduce(event);
            fixture.answer(&mut live, &current.client, effects).await;
        }
    }
    let fresh = fixture.fresh().await;
    fixture.stop().await;
    let id = fixture.agent.id;
    assert_eq!(models(&live, id), models(&fresh, id));
    let repeated = models(&live, id)
        .iter()
        .filter(|m| matches!(m, CellModel::Assistant { text } if text == "after lag"))
        .count();
    assert_eq!(repeated, 1);
}
