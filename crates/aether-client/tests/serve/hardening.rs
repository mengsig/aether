use super::*;

#[tokio::test]
async fn short_secret_refuses_startup_before_binding() {
    let mut weak = hook();
    weak.secret = "x".into();
    let config = ServeConfig {
        address: Some("127.0.0.1:0".parse().unwrap()),
        allow_remote: false,
        replay_path: std::path::PathBuf::new(),
        webhooks: vec![weak],
        subscriptions: Vec::new(),
    };
    let error = aether_client::serve::serve_webhooks(
        Arc::new(Recording::default()),
        &config,
        CancellationToken::new(),
        |_| panic!("a weak secret must never bind"),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("/hooks/ci"), "{error}");
    assert!(error.contains("at least 32 bytes"), "{error}");
    assert!(error.contains("secret_env"), "{error}");
}

#[tokio::test]
async fn unsigned_and_duplicate_headers_name_the_problem() {
    let serving = Serving::start(vec![hook()]).await;
    let (status, answer) = serving
        .post(
            "/hooks/ci",
            &Delivery {
                body: "{}".into(),
                headers: vec![],
            },
        )
        .await;
    assert_eq!(status, 401);
    for name in [
        signature::HEADER,
        signature::HEADER_TIMESTAMP,
        signature::HEADER_NONCE,
    ] {
        assert!(answer.contains(name), "{answer}");
    }
    for name in [
        signature::HEADER,
        signature::HEADER_TIMESTAMP,
        signature::HEADER_NONCE,
        signature::HEADER_DELIVERY_ID,
    ] {
        let mut delivery = Delivery::signed("{}", SECRET);
        if name == signature::HEADER_DELIVERY_ID {
            delivery.headers.push((name, "delivery-1".into()));
        }
        let duplicate = delivery
            .headers
            .iter()
            .find(|(header, _)| *header == name)
            .unwrap()
            .clone();
        delivery.headers.push(duplicate);
        let (status, answer) = serving.post("/hooks/ci", &delivery).await;
        assert_eq!(status, 401, "{answer}");
        assert!(answer.contains("duplicate"), "{answer}");
        assert!(answer.contains(name), "{answer}");
    }
    assert!(serving.sink.triggers().is_empty());
    serving.shutdown.cancel();
}

#[tokio::test]
async fn a_twenty_megabyte_upload_gets_413_signed_or_unsigned() {
    let serving = Serving::start(vec![hook()]).await;
    let body = "x".repeat(20 * 1024 * 1024);
    for delivery in [
        Delivery::signed(&body, SECRET),
        Delivery {
            body,
            headers: vec![],
        },
    ] {
        let (status, answer) = serving.post("/hooks/ci", &delivery).await;
        assert_eq!(status, 413, "{answer}");
        assert!(answer.contains("too large"), "{answer}");
    }
    assert!(serving.sink.triggers().is_empty());
    serving.shutdown.cancel();
}

#[tokio::test]
async fn chunked_uploads_are_bounded_without_content_length() {
    let serving = Serving::start(vec![hook()]).await;
    let delivery = Delivery::signed("{}", SECRET);
    let chunks = futures::stream::iter(
        (0..20).map(|_| Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; 1024 * 1024]))),
    );
    let mut request = reqwest::Client::new()
        .post(format!("http://{}/hooks/ci", serving.address))
        .body(reqwest::Body::wrap_stream(chunks));
    for (name, value) in delivery.headers {
        request = request.header(name, value);
    }
    let response = request
        .send()
        .await
        .expect("a refusal, not a dropped connection");
    assert_eq!(response.status().as_u16(), 413);
    assert!(serving.sink.triggers().is_empty());
    serving.shutdown.cancel();
}

#[tokio::test]
async fn restart_preserves_replay_refusal_with_and_without_delivery_id() {
    for id in [None, Some("ci-delivery-1")] {
        let serving = Serving::start(vec![hook()]).await;
        let mut delivery = Delivery::signed("{}", SECRET);
        if let Some(id) = id {
            delivery
                .headers
                .push((signature::HEADER_DELIVERY_ID, id.into()));
        }
        assert_eq!(serving.post("/hooks/ci", &delivery).await.0, 202);
        serving.shutdown.cancel();
        let restarted = Serving::start_in(vec![hook()], vec![], serving._state).await;
        let (status, answer) = restarted.post("/hooks/ci", &delivery).await;
        assert_eq!(status, 409, "{answer}");
        delivery
            .headers
            .retain(|(name, _)| *name != signature::HEADER_DELIVERY_ID);
        delivery
            .headers
            .push((signature::HEADER_DELIVERY_ID, "changed-id".into()));
        assert_eq!(restarted.post("/hooks/ci", &delivery).await.0, 409);
        assert!(restarted.sink.triggers().is_empty());
        restarted.shutdown.cancel();
    }
}

#[derive(Clone)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn rejected_deliveries_log_reason_and_route_without_credentials() {
    let buffer = LogBuffer(Arc::new(Mutex::new(Vec::new())));
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("only this test installs a global subscriber");
    let mut route = hook();
    route.path = "/hooks/log-check".into();
    let serving = Serving::start(vec![route]).await;
    let unsigned = Delivery {
        body: "{}".into(),
        headers: vec![],
    };
    assert_eq!(serving.post("/hooks/log-check", &unsigned).await.0, 401);
    let mut duplicate = Delivery::signed("{}", SECRET);
    duplicate.headers.push(duplicate.headers[0].clone());
    assert_eq!(serving.post("/hooks/log-check", &duplicate).await.0, 401);
    let huge = Delivery {
        body: "x".repeat(20 * 1024 * 1024),
        headers: vec![],
    };
    assert_eq!(serving.post("/hooks/log-check", &huge).await.0, 413);
    let logs = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    let rejected: Vec<_> = logs
        .lines()
        .filter(|line| line.contains("webhook rejected") && line.contains("/hooks/log-check"))
        .collect();
    assert_eq!(rejected.len(), 3, "{logs}");
    assert!(rejected.iter().all(|line| line.contains("reason=")));
    assert!(
        rejected
            .iter()
            .any(|line| line.contains("missing required headers"))
    );
    assert!(
        rejected
            .iter()
            .any(|line| line.contains("duplicate header"))
    );
    assert!(
        rejected
            .iter()
            .any(|line| line.contains("body exceeds limit"))
    );
    assert!(!logs.contains(SECRET));
    assert!(!logs.contains(&duplicate.headers[0].1));
    serving.shutdown.cancel();
}
