use super::*;

#[tokio::test]
async fn the_http_endpoint_refuses_bad_tokens_throttles_guessing_and_never_locks_out_a_valid_one() {
    let sink: Arc<dyn aether_client::mcp::CommandSink> = Arc::new(RecordingSink::default());
    let shutdown = tokio_util::sync::CancellationToken::new();
    let (bound, serving) = serve_http(sink, shutdown.clone()).await;
    let url = format!("http://{bound}/");
    let http = reqwest::Client::new();
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}});

    let refused = http
        .post(&url)
        .json(&body)
        .send()
        .await
        .expect("request without a token");
    assert_eq!(refused.status(), reqwest::StatusCode::UNAUTHORIZED);

    let good = |http: &reqwest::Client| {
        http.post(&url)
            .header("authorization", format!("Bearer {TOKEN}"))
            .json(&body)
            .send()
    };
    // One bad attempt must not lock out the valid client behind it.
    let ok = good(&http).await.expect("valid after one failure");
    assert_eq!(ok.status(), reqwest::StatusCode::OK);

    // Repeated guesses are throttled, yet the valid token is still served.
    let mut statuses = Vec::new();
    for _ in 0..8 {
        let started = std::time::Instant::now();
        let bad = http
            .post(&url)
            .header("authorization", "Bearer nope")
            .json(&body)
            .send()
            .await
            .expect("bad attempt");
        if bad.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            assert!(
                started.elapsed() >= std::time::Duration::from_millis(400),
                "a throttled guess must be paced, not answered at full speed"
            );
        }
        statuses.push(bad.status());
    }
    assert_eq!(statuses[0], reqwest::StatusCode::UNAUTHORIZED);
    assert_eq!(
        *statuses.last().expect("attempts"),
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        "repeated failures must be throttled"
    );
    let ok = good(&http).await.expect("valid while throttled");
    assert_eq!(ok.status(), reqwest::StatusCode::OK);

    shutdown.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), serving).await;
}

#[tokio::test]
async fn the_http_endpoint_serves_the_operator_token_and_refuses_a_browser() {
    let sink: Arc<dyn aether_client::mcp::CommandSink> = Arc::new(RecordingSink::default());
    let shutdown = tokio_util::sync::CancellationToken::new();
    let (bound, serving) = serve_http(sink, shutdown.clone()).await;
    let url = format!("http://{bound}/");
    let http = reqwest::Client::new();
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}});

    // RFC 7235 makes the scheme case-insensitive.
    let ok: Value = http
        .post(&url)
        .header("authorization", format!("bearer {TOKEN}"))
        .json(&body)
        .send()
        .await
        .expect("lowercase scheme")
        .json()
        .await
        .expect("json body");
    assert!(
        ok["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .any(|t| t["name"] == "status")
    );

    // A browser is never a legitimate caller here, token or not: this is what
    // closes DNS rebinding against a locally bound endpoint.
    let from_a_page = http
        .post(&url)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("origin", "http://evil.example")
        .json(&body)
        .send()
        .await
        .expect("browser request");
    assert_eq!(from_a_page.status(), reqwest::StatusCode::FORBIDDEN);

    shutdown.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), serving).await;
}

#[tokio::test]
async fn a_weak_token_and_a_public_bind_are_refused_before_the_port_opens() {
    let sink: Arc<dyn aether_client::mcp::CommandSink> = Arc::new(RecordingSink::default());
    let shutdown = tokio_util::sync::CancellationToken::new();

    let weak = aether_client::mcp::http::HttpConfig {
        token: "s3cret".into(),
        ..config("127.0.0.1:0")
    };
    let e = aether_client::mcp::http::serve(sink.clone(), weak, shutdown.clone(), |_| {})
        .await
        .expect_err("a guessable token must not serve the fleet");
    assert!(e.to_string().contains("characters"), "{e}");

    // 0.0.0.0 is every interface, so it is not loopback: meaning it has to be
    // said, because Aether terminates no TLS.
    let public = aether_client::mcp::http::HttpConfig {
        ..config("0.0.0.0:0")
    };
    let e = aether_client::mcp::http::serve(sink.clone(), public, shutdown.clone(), |_| {})
        .await
        .expect_err("a public bind must not be the default");
    assert!(e.to_string().contains("--allow-remote"), "{e}");

    let deliberate = aether_client::mcp::http::HttpConfig {
        allow_remote: true,
        ..config("0.0.0.0:0")
    };
    let stop = shutdown.clone();
    let serving = tokio::spawn(async move {
        aether_client::mcp::http::serve(sink, deliberate, stop, |_| {}).await
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    shutdown.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), serving).await;
}
