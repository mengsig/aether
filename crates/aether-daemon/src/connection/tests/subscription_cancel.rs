use super::*;
use aether_core::event::{EventDraft, EventKind};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn unsubscribe_stops_replay_with_a_full_output_queue() {
    let (daemon, _config) = daemon().await;
    let mut commit = aether_store::Commit::new();
    for _ in 0..501 {
        commit = commit.event(EventDraft::new(Actor::System, EventKind::AgentReady));
    }
    daemon.runtime.apply(commit).expect("replay history");
    let daemon = Arc::new(daemon);
    let (out, mut received) = mpsc::channel(1);
    let (failure, _failures) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    spawn_subscription(
        daemon.clone(),
        out,
        7,
        Some(0),
        Default::default(),
        false,
        SubscriptionControl {
            auth: AuthContext::Local,
            cancel: cancel.clone(),
            failure,
        },
    )
    .expect("subscribe");
    // This task cannot advance until the producer has filled its sole slot.
    let first = received.recv().await.expect("first event");
    assert!(matches!(first, ServerFrame::Event { sub: 7, .. }));
    cancel.cancel();
    // A second event may have won delivery before cancellation. No more may
    // be produced, even though the output channel remains open and writable.
    let ended = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut remaining = 0;
        while received.recv().await.is_some() {
            remaining += 1;
        }
        remaining
    })
    .await
    .expect("cancelled replay task releases its sender");
    assert!(
        ended <= 1,
        "replay delivered {ended} frames after cancellation"
    );
    daemon.runtime.shutdown().await;
}

#[tokio::test]
async fn subscription_accounting_is_released_when_a_task_ends_itself() {
    let (daemon, _config) = daemon().await;
    let daemon = Arc::new(daemon);
    let (out, received) = mpsc::channel(1);
    let (failure, _failures) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    let (_, task) = spawn_subscription(
        daemon.clone(),
        out,
        1,
        Some(0),
        Default::default(),
        false,
        SubscriptionControl {
            auth: AuthContext::Local,
            cancel: cancel.clone(),
            failure,
        },
    )
    .unwrap();
    let mut subscriptions = HashMap::from([(1, (cancel.clone(), task))]);
    assert_eq!(subscriptions.len(), 1);
    drop(received);
    daemon
        .runtime
        .apply(
            aether_store::Commit::new()
                .event(EventDraft::new(Actor::System, EventKind::AgentReady)),
        )
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !subscriptions[&1].1.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("closed receiver ends the task without unsubscribe");
    assert!(!cancel.is_cancelled());
    reap_subscriptions(&mut subscriptions).await.unwrap();
    assert!(subscriptions.is_empty());
    daemon.runtime.shutdown().await;
}
