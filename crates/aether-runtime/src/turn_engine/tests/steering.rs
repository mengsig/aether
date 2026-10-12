use super::*;

#[tokio::test]
async fn full_steering_channel_preserves_messages_without_blocking() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let mut ctx = started_agent_ctx(&runtime).await;
    let cursor = ctx.agent.inbox_cursor;
    let mut steering = Steering::new(cursor);
    let (tx, mut rx) = mpsc::channel(1);
    tx.try_send("already queued".into()).expect("fill input");
    let mut input = Some(tx);
    runtime
        .send_message(
            ctx.agent.id,
            vec![ContentBlock::text("status?")],
            Actor::user(),
        )
        .expect("deliver message");

    tokio::time::timeout(
        std::time::Duration::from_millis(100),
        steer(&runtime, &mut ctx, &mut steering, &mut input),
    )
    .await
    .expect("full input must not suspend the event pump")
    .expect("steer");
    assert!(input.is_some());
    assert!(steering.outstanding.is_empty());
    assert_eq!(steering.sent_up_to, cursor);
    assert_eq!(ctx.agent.inbox_cursor, cursor);
    assert!(!ctx.user_reply_pending);
    assert!(steering.retry);

    runtime
        .send_message(
            ctx.agent.id,
            vec![ContentBlock::text("second message")],
            Actor::user(),
        )
        .expect("deliver another message while full");

    assert_eq!(rx.recv().await, Some("already queued".into()));
    steer(&runtime, &mut ctx, &mut steering, &mut input)
        .await
        .expect("retry steering");
    let text = rx.recv().await.expect("message").text;
    assert!(
        text.find("status?").expect("first message")
            < text.find("second message").expect("second message")
    );
    assert!(!steering.retry);
    steer(&runtime, &mut ctx, &mut steering, &mut input)
        .await
        .expect("steer again without new inbox");
    assert!(matches!(
        rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    assert_eq!(steering.outstanding.len(), 1);
    assert_eq!(ctx.agent.inbox_cursor, cursor);
    assert!(ctx.user_reply_pending);
    let pending = steering
        .outstanding
        .pop_front()
        .expect("awaiting acceptance");
    record_consumed(&runtime, &mut ctx, pending).expect("acceptance");
    assert!(ctx.agent.inbox_cursor > cursor);
    runtime.shutdown().await;
}

#[tokio::test]
async fn closed_steering_channel_leaves_inbox_unconsumed() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let mut ctx = started_agent_ctx(&runtime).await;
    let cursor = ctx.agent.inbox_cursor;
    let mut steering = Steering::new(cursor);
    let (tx, rx) = mpsc::channel(1);
    drop(rx);
    let mut input = Some(tx);
    runtime
        .send_message(
            ctx.agent.id,
            vec![ContentBlock::text("queued")],
            Actor::user(),
        )
        .expect("deliver message");
    steer(&runtime, &mut ctx, &mut steering, &mut input)
        .await
        .expect("steer");
    assert!(input.is_none());
    assert!(!steering.retry);
    assert!(steering.outstanding.is_empty());
    assert_eq!(ctx.agent.inbox_cursor, cursor);
    assert!(
        steering
            .pending(&runtime, &ctx)
            .expect("pending inbox")
            .is_some()
    );
    runtime.shutdown().await;
}
