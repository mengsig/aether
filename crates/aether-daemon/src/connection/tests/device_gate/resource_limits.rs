use super::*;

#[tokio::test]
async fn observer_subscription_cap_is_released_on_unsubscribe() {
    let mut carrier = Carrier::open(DeviceRole::Observer).await;
    carrier.greet().await;
    let subscribe = || Command::Subscribe {
        after: None,
        query: Default::default(),
        deltas: true,
    };
    let mut subscriptions = Vec::new();
    for req in 2..2 + MAX_SUBSCRIPTIONS as u64 {
        let reply = carrier.request(req, subscribe()).await.unwrap();
        let Reply::Subscribed { id, .. } = reply else {
            panic!("subscription reply")
        };
        subscriptions.push(id);
    }
    assert_eq!(subscriptions.len(), MAX_SUBSCRIPTIONS);
    let error = carrier.request(30, subscribe()).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::Unavailable);
    assert!(error.message.contains("16"));
    assert_eq!(
        carrier
            .request(
                31,
                Command::Unsubscribe {
                    id: subscriptions[0]
                }
            )
            .await,
        Ok(Reply::Ok)
    );
    assert!(matches!(
        carrier.request(32, subscribe()).await,
        Ok(Reply::Subscribed { .. })
    ));
    carrier.writer.shutdown().await.unwrap();
    assert_eq!(carrier.ended().await, Ended::Closed);
}

#[tokio::test]
async fn revocation_interrupts_the_daemon_writer_with_a_large_pending_snapshot() {
    use tokio::io::AsyncReadExt;
    let mut carrier = Carrier::open(DeviceRole::Observer).await;
    carrier.greet().await;
    store_large_snapshot(&carrier.runtime);
    let line = aether_protocol::encode(&ClientFrame {
        id: 2,
        command: Command::GetEvents {
            after: 0,
            limit: 10,
            query: Default::default(),
        },
    })
    .unwrap();
    carrier.writer.write_all(line.as_bytes()).await.unwrap();
    let mut first = [0; 1];
    carrier
        .lines
        .get_mut()
        .read_exact(&mut first)
        .await
        .unwrap();
    assert_eq!(first[0], b'{', "the snapshot is in flight");
    carrier.sessions.close(carrier.identity.id);
    let ended = tokio::time::timeout(Duration::from_millis(500), &mut carrier.serving)
        .await
        .expect("the writer is cancelled without reader progress")
        .unwrap()
        .unwrap();
    assert_eq!(ended, Ended::Revoked);
    let mut remaining = Vec::new();
    carrier
        .lines
        .get_mut()
        .read_to_end(&mut remaining)
        .await
        .unwrap();
    assert!(
        remaining.len() < 128 * 1024,
        "only transport and reader buffers remain"
    );
    assert!(
        !remaining.ends_with(b"\n"),
        "the pending snapshot never completes"
    );
    carrier.runtime.shutdown().await;
}

fn store_large_snapshot(runtime: &aether_runtime::Runtime) {
    runtime
        .apply(
            aether_store::Commit::new().event(aether_core::event::EventDraft::new(
                Actor::System,
                aether_core::event::EventKind::Custom {
                    name: "large.snapshot".into(),
                    payload: serde_json::json!({"text": "x".repeat(1024 * 1024)}),
                },
            )),
        )
        .unwrap();
    assert!(runtime.store().last_seq().unwrap() > 0);
}
