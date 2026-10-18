use super::*;
use aether_client::{MAX_QUEUED_SUBSCRIPTION_FRAMES, Transport};
use aether_core::event::{Actor, Event, EventDraft, EventKind};
use aether_core::id::EventId;
use aether_core::time::now;
use aether_protocol::{Command, Reply, ServerFrame};
use futures::StreamExt;

#[tokio::test]
async fn webhook_delivery_replays_every_undelivered_event_after_queue_overflow() {
    let (transport, mut peer) = Transport::memory(16);
    let client = Client::from_transport(transport);
    let initial_seq = 40;
    let last_buffered = initial_seq + MAX_QUEUED_SUBSCRIPTION_FRAMES as u64;
    let last_emitted = last_buffered + 3;
    let (overflowed, overflow_seen) = tokio::sync::oneshot::channel();
    let (delivered, delivered_all) = tokio::sync::oneshot::channel();
    let daemon = tokio::spawn(async move {
        let request = peer.incoming.recv().await.expect("subscribe");
        assert!(matches!(
            request.command,
            Command::Subscribe {
                after: None,
                deltas: false,
                ..
            }
        ));
        peer.outgoing
            .send(ServerFrame::Reply {
                id: request.id,
                result: Ok(Reply::Subscribed {
                    id: 7,
                    last_seq: initial_seq,
                }),
            })
            .await
            .expect("subscribed");
        for seq in initial_seq + 1..=last_emitted {
            peer.outgoing.send(event(7, seq)).await.expect("event");
        }
        let cleanup = peer.incoming.recv().await.expect("overflow cleanup");
        assert!(matches!(cleanup.command, Command::Unsubscribe { id: 7 }));
        peer.outgoing
            .send(ServerFrame::Reply {
                id: cleanup.id,
                result: Ok(Reply::Ok),
            })
            .await
            .expect("released");
        overflowed.send(()).expect("consumer waiting");
        let request = peer.incoming.recv().await.expect("resume");
        assert!(
            matches!(request.command, Command::Subscribe { after: Some(seq), deltas: false, .. } if seq == last_buffered)
        );
        peer.outgoing
            .send(ServerFrame::Reply {
                id: request.id,
                result: Ok(Reply::Subscribed {
                    id: 8,
                    last_seq: last_emitted,
                }),
            })
            .await
            .expect("resubscribed");
        for seq in last_buffered + 1..=last_emitted + 1 {
            peer.outgoing
                .send(event(8, seq))
                .await
                .expect("replay and live event");
        }
        delivered_all.await.expect("consumer finished");
    });
    let sub = client
        .subscribe(None, Default::default(), false)
        .await
        .expect("subscription");
    let stream = serve_events(client, Some(sub));
    futures::pin_mut!(stream);
    overflow_seen
        .await
        .expect("queue overflowed while delivery was blocked");
    for seq in initial_seq + 1..=last_emitted + 1 {
        assert_eq!(stream.next().await.expect("delivery continues").seq, seq);
    }
    delivered.send(()).expect("daemon waiting");
    daemon.await.expect("daemon finished");
}

fn event(sub: u64, seq: u64) -> ServerFrame {
    ServerFrame::Event {
        sub,
        event: Event::from_draft(
            EventDraft::new(Actor::System, EventKind::AgentReady),
            EventId::new(),
            seq,
            now(),
        ),
    }
}

#[test]
fn short_webhook_secret_names_the_route_and_environment_fix() {
    let error = super::validate_webhook_secret("x", "AETHER_TEST_HOOK_SECRET", "/hooks/ci")
        .expect_err("short secrets must fail before connecting to a daemon");
    let message = error.to_string();
    assert!(message.contains("AETHER_TEST_HOOK_SECRET"));
    assert!(message.contains("/hooks/ci"));
    assert!(message.contains("at least 32 bytes"));
    assert!(super::validate_webhook_secret(&"x".repeat(32), "SECRET", "/hook").is_ok());
}
