use super::*;

#[tokio::test]
async fn a_signed_post_becomes_a_trigger_and_an_unsigned_one_does_not() {
    let serving = Serving::start(vec![hook()]).await;
    let body = r#"{"ref":"main","status":"failed"}"#;

    let (status, answer) = serving
        .post("/hooks/ci", &Delivery::signed(body, SECRET))
        .await;
    assert_eq!(status, 202, "{answer}");
    let triggers = serving.sink.triggers();
    assert_eq!(triggers.len(), 1, "{triggers:?}");
    let Command::Trigger {
        source, payload, ..
    } = &triggers[0]
    else {
        panic!("not a trigger: {triggers:?}");
    };
    assert_eq!(source, "ci");
    assert_eq!(payload["ref"], "main", "the body arrives as the payload");

    // Each of these is correctly stamped and wrongly signed, so what is being
    // refused is the signature and not the stamp.
    for presented in [
        None,
        Some(String::new()),
        // Another secret over the right body.
        signature::sign(
            "the wrong secret",
            &signature::signed_payload(&signature::Stamp::now(), body.as_bytes()),
        ),
        // The right secret over a different body.
        signature::sign(
            SECRET,
            &signature::signed_payload(&signature::Stamp::now(), b"{}"),
        ),
    ] {
        let delivery = Delivery::signed(body, SECRET).with_signature(presented.as_deref());
        let (status, answer) = serving.post("/hooks/ci", &delivery).await;
        assert_eq!(status, 401, "{answer}");
    }
    // And one with a good signature over the body alone, which is what a
    // caller written against the unstamped scheme would send.
    let unstamped = Delivery {
        body: body.to_owned(),
        headers: vec![(
            signature::HEADER,
            signature::sign(SECRET, body.as_bytes()).expect("signed"),
        )],
    };
    let (status, answer) = serving.post("/hooks/ci", &unstamped).await;
    assert_eq!(status, 401, "{answer}");
    assert_eq!(
        serving.sink.triggers().len(),
        1,
        "an unsigned post must not reach the daemon"
    );
    serving.shutdown.cancel();
}

/// The control the architecture claims: one captured POST, sent twice.
#[tokio::test]
async fn a_valid_delivery_replayed_byte_for_byte_is_refused() {
    let serving = Serving::start(vec![hook()]).await;
    let delivery = Delivery::signed(r#"{"ref":"main"}"#, SECRET);

    let (status, answer) = serving.post("/hooks/ci", &delivery).await;
    assert_eq!(status, 202, "{answer}");
    let (status, answer) = serving.post("/hooks/ci", &delivery).await;
    assert_eq!(status, 409, "the same bytes again: {answer}");
    assert!(answer.contains("already accepted"), "{answer}");
    assert_eq!(
        serving.sink.triggers().len(),
        1,
        "a replay must not wake the agent a second time"
    );
    serving.shutdown.cancel();
}

/// A capture kept past the window is refused on age alone, whether or not the
/// nonce is still remembered.
#[tokio::test]
async fn a_delivery_stamped_outside_the_window_is_refused() {
    let serving = Serving::start(vec![hook()]).await;
    let window = aether_client::serve::replay::REPLAY_WINDOW.as_secs() as i64;
    let now = aether_core::time::now().timestamp();
    for timestamp in [now - window - 60, now + window + 60] {
        let stamp = signature::Stamp {
            timestamp,
            nonce: format!("n{timestamp}"),
        };
        let delivery = Delivery::stamped("{}", SECRET, stamp);
        let (status, answer) = serving.post("/hooks/ci", &delivery).await;
        assert_eq!(status, 409, "{answer}");
    }
    assert!(serving.sink.triggers().is_empty());
    serving.shutdown.cancel();
}

/// A signature with no stamp to cover cannot be checked at all, so it is
/// refused before the body is read.
#[tokio::test]
async fn a_delivery_without_a_stamp_is_refused() {
    let serving = Serving::start(vec![hook()]).await;
    let delivery = Delivery::signed("{}", SECRET).without_stamp();
    let (status, answer) = serving.post("/hooks/ci", &delivery).await;
    assert_eq!(status, 401, "{answer}");
    assert!(answer.contains("x-aether-timestamp"), "{answer}");
    assert!(serving.sink.triggers().is_empty());
    serving.shutdown.cancel();
}

#[tokio::test]
async fn an_unconfigured_path_and_a_get_are_refused() {
    let serving = Serving::start(vec![hook()]).await;
    let (status, _) = serving
        .post("/hooks/other", &Delivery::signed("{}", SECRET))
        .await;
    assert_eq!(status, 404);
    let status = reqwest::Client::new()
        .get(format!("http://{}/hooks/ci", serving.address))
        .send()
        .await
        .expect("get")
        .status();
    assert_eq!(status.as_u16(), 405);
    assert!(serving.sink.triggers().is_empty());
    serving.shutdown.cancel();
}

/// A body that is signed but not JSON is a caller's mistake, and it is named
/// rather than turned into a trigger with an empty payload.
#[tokio::test]
async fn a_signed_body_that_is_not_json_is_refused() {
    let serving = Serving::start(vec![hook()]).await;
    let (status, answer) = serving
        .post("/hooks/ci", &Delivery::signed("not json", SECRET))
        .await;
    assert_eq!(status, 400, "{answer}");
    assert!(answer.contains("not JSON"), "{answer}");
    assert!(serving.sink.triggers().is_empty());
    serving.shutdown.cancel();
}

/// A hook configured by name wakes the one live agent with that name - and
/// refuses rather than guessing when the name is not unique, or when the only
/// agent that had it has already finished.
#[tokio::test]
async fn a_hook_resolves_a_name_against_live_agents_only() {
    let by_name = || Webhook {
        agent: "builder".into(),
        ..hook()
    };

    let one = named_agent("builder", AgentStatus::Running);
    let expected = one.id;
    let serving = Serving::start_with(vec![by_name()], vec![one]).await;
    let (status, answer) = serving
        .post("/hooks/ci", &Delivery::signed("{}", SECRET))
        .await;
    assert_eq!(status, 202, "{answer}");
    assert!(answer.contains(&expected.to_string()), "{answer}");
    let triggers = serving.sink.triggers();
    let Command::Trigger { agent, .. } = &triggers[0] else {
        panic!("not a trigger: {triggers:?}");
    };
    assert_eq!(*agent, expected, "the trigger names the agent that matched");
    serving.shutdown.cancel();

    let two = vec![
        named_agent("builder", AgentStatus::Running),
        named_agent("builder", AgentStatus::Ready),
    ];
    let serving = Serving::start_with(vec![by_name()], two).await;
    let (status, answer) = serving
        .post("/hooks/ci", &Delivery::signed("{}", SECRET))
        .await;
    assert_eq!(status, 404, "{answer}");
    assert!(answer.contains("more than one"), "{answer}");
    assert!(serving.sink.triggers().is_empty());
    serving.shutdown.cancel();

    // The agent that had the name is gone. Before `live_only` this both woke
    // a finished agent and, once a replacement existed, refused every POST as
    // ambiguous for the life of the store.
    let dead = vec![named_agent(
        "builder",
        AgentStatus::Completed { summary: None },
    )];
    let serving = Serving::start_with(vec![by_name()], dead).await;
    let (status, answer) = serving
        .post("/hooks/ci", &Delivery::signed("{}", SECRET))
        .await;
    assert_eq!(status, 404, "{answer}");
    assert!(answer.contains("no live agent"), "{answer}");
    assert!(serving.sink.triggers().is_empty());
    serving.shutdown.cancel();
}

/// A hook configured by id needs no lookup at all.
#[tokio::test]
async fn a_hook_configured_by_id_triggers_that_id() {
    let serving = Serving::start(vec![hook()]).await;
    let (status, answer) = serving
        .post("/hooks/ci", &Delivery::signed("{}", SECRET))
        .await;
    assert_eq!(status, 202, "{answer}");
    assert!(
        !serving
            .sink
            .seen
            .lock()
            .expect("lock")
            .iter()
            .any(|c| matches!(c, Command::ListAgents { .. })),
        "an id is not looked up"
    );
    serving.shutdown.cancel();
}

/// The bad-signature delay and body read used to hold a
/// general connection slot for their whole duration, so a flood of
/// unverified requests could deny the listener at up to `MAX_CONNECTIONS`
/// (32) at once. They now hold a smaller, dedicated `PRE_AUTH_SLOTS` budget -
/// this proves that budget is enforced: sending more unverified deliveries
/// at once than it holds gets the extras refused immediately rather than
/// each costing a full `BAD_SIGNATURE_DELAY`.
#[tokio::test]
async fn more_unverified_deliveries_than_the_pre_auth_pool_are_refused_at_once() {
    let serving = Serving::start(vec![hook()]).await;
    let concurrency = aether_client::serve::PRE_AUTH_SLOTS + 4;
    let mut handles = Vec::new();
    for _ in 0..concurrency {
        let address = serving.address;
        let delivery = Delivery::signed("{}", SECRET).with_signature(Some("wrong-signature"));
        handles.push(tokio::spawn(async move {
            let mut request = reqwest::Client::new()
                .post(format!("http://{address}/hooks/ci"))
                .body(delivery.body.clone());
            for (name, value) in &delivery.headers {
                request = request.header(*name, value);
            }
            request.send().await.expect("post").status().as_u16()
        }));
    }
    let mut refused_at_once = 0;
    let mut refused_after_the_delay = 0;
    for handle in handles {
        match handle.await.expect("joined") {
            503 => refused_at_once += 1,
            401 => refused_after_the_delay += 1,
            other => panic!("unexpected status {other}"),
        }
    }
    assert_eq!(
        refused_at_once,
        concurrency - aether_client::serve::PRE_AUTH_SLOTS,
        "only the pre-auth pool's worth of requests should pay the bad-signature delay"
    );
    assert_eq!(
        refused_after_the_delay,
        aether_client::serve::PRE_AUTH_SLOTS
    );
    serving.shutdown.cancel();
}

#[tokio::test]
async fn shared_route_secrets_are_refused_before_listening() {
    let first = hook();
    let mut second = hook();
    second.path = "/hooks/other".into();
    let config = ServeConfig {
        address: Some("127.0.0.1:0".parse().unwrap()),
        allow_remote: false,
        replay_path: std::path::PathBuf::new(),
        webhooks: vec![first, second],
        subscriptions: Vec::new(),
    };
    let error = aether_client::serve::serve_webhooks(
        Arc::new(Recording::default()),
        &config,
        CancellationToken::new(),
        |_| panic!("must refuse before binding"),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("/hooks/ci"));
    assert!(error.to_string().contains("/hooks/other"));
    assert!(!error.to_string().contains(SECRET));
}

#[tokio::test]
async fn a_delivery_cannot_authenticate_on_another_route() {
    let first = hook();
    let mut second = hook();
    second.path = "/hooks/other".into();
    second.secret = "another-route-test-only-key-32-bytes-long".into();
    let serving = Serving::start(vec![first, second]).await;
    let delivery = Delivery::signed("{}", SECRET);
    assert_eq!(serving.post("/hooks/ci", &delivery).await.0, 202);
    assert_eq!(serving.post("/hooks/other", &delivery).await.0, 401);
    assert_eq!(serving.sink.triggers().len(), 1);
    serving.shutdown.cancel();
}

#[tokio::test]
async fn an_outbound_signing_key_cannot_authenticate_an_inbound_route() {
    let config = ServeConfig {
        address: Some("127.0.0.1:0".parse().unwrap()),
        allow_remote: false,
        replay_path: std::path::PathBuf::new(),
        webhooks: vec![hook()],
        subscriptions: vec![Subscription {
            url: "https://receiver.example/events".into(),
            events: Vec::new(),
            secret: SECRET.into(),
            max_attempts: 1,
            dead_letter: None,
        }],
    };
    let error = aether_client::serve::serve_webhooks(
        Arc::new(Recording::default()),
        &config,
        CancellationToken::new(),
        |_| panic!("must refuse before binding"),
    )
    .await
    .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("/hooks/ci"));
    assert!(message.contains("https://receiver.example/events"));
    assert!(!message.contains(SECRET));
}
