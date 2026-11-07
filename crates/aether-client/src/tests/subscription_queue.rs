use super::*;

#[tokio::test]
async fn a_slow_subscription_reports_lag_without_blocking_replies_and_can_resume() {
    let (transport, mut peer) = Transport::memory(16);
    let client = Client::from_transport(transport);
    let subscribing = tokio::spawn({
        let client = client.clone();
        async move { client.subscribe(Some(0), EventQuery::default(), true).await }
    });
    let request = peer.incoming.recv().await.unwrap();
    peer.outgoing
        .send(ServerFrame::Reply {
            id: request.id,
            result: Ok(Reply::Subscribed { id: 7, last_seq: 0 }),
        })
        .await
        .unwrap();
    let mut sub = subscribing.await.unwrap().unwrap();
    for seq in 1..=MAX_QUEUED_SUBSCRIPTION_FRAMES as u64 + 2 {
        peer.outgoing
            .send(ServerFrame::Event {
                sub: 7,
                event: Event::from_draft(
                    EventDraft::new(Actor::System, EventKind::AgentReady),
                    EventId::new(),
                    seq,
                    now(),
                ),
            })
            .await
            .unwrap();
    }
    let pinging = tokio::spawn({
        let client = client.clone();
        async move { client.request(Command::Ping).await }
    });
    let mut saw_cleanup = false;
    let mut saw_ping = false;
    for _ in 0..2 {
        let request = peer.incoming.recv().await.unwrap();
        let reply = match request.command {
            Command::Unsubscribe { id: 7 } => {
                saw_cleanup = true;
                Reply::Ok
            }
            Command::Ping => {
                saw_ping = true;
                Reply::Ok
            }
            other => panic!("unexpected request: {other:?}"),
        };
        peer.outgoing
            .send(ServerFrame::Reply {
                id: request.id,
                result: Ok(reply),
            })
            .await
            .unwrap();
    }
    assert!(saw_cleanup && saw_ping);
    assert!(matches!(pinging.await.unwrap().unwrap(), Reply::Ok));
    let mut last = 0;
    for _ in 0..MAX_QUEUED_SUBSCRIPTION_FRAMES {
        let Some(SubscriptionFrame::Event(event)) = sub.next().await else {
            panic!("buffered event")
        };
        assert_eq!(event.seq, last + 1);
        last = event.seq;
    }
    assert_eq!(
        futures::StreamExt::next(&mut sub).await,
        Some(SubscriptionFrame::Lagged {
            capacity: MAX_QUEUED_SUBSCRIPTION_FRAMES
        })
    );
    assert!(sub.next().await.is_none());
    let resuming = tokio::spawn({
        let client = client.clone();
        async move {
            client
                .subscribe(Some(last), EventQuery::default(), false)
                .await
        }
    });
    let request = peer.incoming.recv().await.unwrap();
    assert!(matches!(request.command, Command::Subscribe { after: Some(seq), .. } if seq == last));
    peer.outgoing
        .send(ServerFrame::Reply {
            id: request.id,
            result: Ok(Reply::Subscribed {
                id: 8,
                last_seq: last,
            }),
        })
        .await
        .unwrap();
    let mut resumed = resuming.await.unwrap().unwrap();
    peer.outgoing
        .send(ServerFrame::Event {
            sub: 8,
            event: Event::from_draft(
                EventDraft::new(Actor::System, EventKind::AgentReady),
                EventId::new(),
                last + 1,
                now(),
            ),
        })
        .await
        .unwrap();
    assert!(
        matches!(resumed.next().await, Some(SubscriptionFrame::Event(event)) if event.seq == last + 1)
    );
}

#[tokio::test]
async fn generic_subscribe_requests_are_refused_before_allocating_a_stream() {
    let (transport, mut peer) = Transport::memory(16);
    let client = Client::from_transport(transport);
    let error = client
        .request(Command::Subscribe {
            after: None,
            query: EventQuery::default(),
            deltas: false,
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Client::subscribe"));
    assert!(matches!(error, ClientError::InvalidUsage(_)));
    assert!(peer.incoming.try_recv().is_err());
    assert!(client.shared.subscriptions.lock().await.is_empty());
    assert!(client.shared.sub_receivers.lock().await.is_empty());
}

#[tokio::test]
async fn overflow_before_subscribe_claims_its_receiver_remains_observable() {
    let (transport, mut peer) = Transport::memory(16);
    let client = Client::from_transport(transport);
    let mut subscribing = Box::pin(client.subscribe(Some(0), EventQuery::default(), true));
    assert!(futures::poll!(&mut subscribing).is_pending());
    let request = peer.incoming.recv().await.unwrap();
    peer.outgoing
        .send(ServerFrame::Reply {
            id: request.id,
            result: Ok(Reply::Subscribed { id: 7, last_seq: 0 }),
        })
        .await
        .unwrap();
    for _ in 0..=MAX_QUEUED_SUBSCRIPTION_FRAMES {
        peer.outgoing
            .send(ServerFrame::Delta {
                invocation_seq: None,
                sub: 7,
                agent: AgentId::new(),
                invocation: InvocationId::new(),
                delta: Delta::Text {
                    text: "delta".into(),
                },
            })
            .await
            .unwrap();
    }
    let cleanup = peer.incoming.recv().await.unwrap();
    assert!(matches!(cleanup.command, Command::Unsubscribe { id: 7 }));
    peer.outgoing
        .send(ServerFrame::Reply {
            id: cleanup.id,
            result: Ok(Reply::Ok),
        })
        .await
        .unwrap();
    let mut sub = subscribing.await.unwrap();
    for _ in 0..MAX_QUEUED_SUBSCRIPTION_FRAMES {
        assert!(matches!(
            sub.next().await,
            Some(SubscriptionFrame::Delta { .. })
        ));
    }
    assert_eq!(
        sub.next().await,
        Some(SubscriptionFrame::Lagged {
            capacity: MAX_QUEUED_SUBSCRIPTION_FRAMES
        })
    );
    assert!(sub.next().await.is_none());
}

#[tokio::test]
async fn a_subscription_confirmed_just_before_disconnect_ends_instead_of_failing() {
    let (transport, mut peer) = Transport::memory(16);
    let client = Client::from_transport(transport);
    let subscribing = tokio::spawn({
        let client = client.clone();
        async move { client.subscribe(None, EventQuery::default(), false).await }
    });
    let request = peer.incoming.recv().await.unwrap();
    peer.outgoing
        .send(ServerFrame::Reply {
            id: request.id,
            result: Ok(Reply::Subscribed { id: 7, last_seq: 0 }),
        })
        .await
        .unwrap();
    // The reader sees the reply and the close in one pass, before the
    // caller claims its channel.
    drop(peer);
    let mut sub = subscribing.await.unwrap().unwrap();
    assert!(sub.next().await.is_none());
}
