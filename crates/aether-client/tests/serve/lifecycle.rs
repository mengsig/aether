use super::*;

fn serve_config(webhooks: Vec<Webhook>, subscriptions: Vec<Subscription>) -> ServeConfig {
    ServeConfig {
        address: Some("127.0.0.1:0".parse().expect("addr")),
        allow_remote: false,
        replay_path: std::path::PathBuf::new(),
        webhooks,
        subscriptions,
    }
}

/// The inbound half had the configuration error in hand; sequencing the two
/// halves meant it waited on an event stream that never ends before reporting
/// it, so `aether serve` hung instead of saying what was wrong.
#[tokio::test]
async fn a_configuration_error_in_one_half_is_reported_while_the_other_runs_forever() {
    let config = serve_config(
        vec![hook(), hook()],
        vec![Subscription {
            url: "http://127.0.0.1:9/hook".into(),
            events: Vec::new(),
            secret: SECRET.into(),
            max_attempts: 1,
            dead_letter: None,
        }],
    );
    let failed = tokio::time::timeout(
        Duration::from_secs(5),
        aether_client::serve::run(
            Arc::new(Recording::default()),
            config,
            futures::stream::pending::<Event>(),
            CancellationToken::new(),
            |_| {},
        ),
    )
    .await
    .expect("the error is reported, not waited on")
    .expect_err("two webhooks on one path");
    assert!(failed.to_string().contains("webhook listener"), "{failed}");
}

/// The daemon connection dropping ends the event stream. That is not a clean
/// stop: it is this process silently no longer delivering anything.
#[tokio::test]
async fn a_daemon_disconnect_ends_the_run_with_an_error_not_a_green_exit() {
    let config = ServeConfig {
        address: None,
        ..serve_config(
            Vec::new(),
            vec![Subscription {
                url: "http://127.0.0.1:9/hook".into(),
                events: Vec::new(),
                secret: SECRET.into(),
                max_attempts: 1,
                dead_letter: None,
            }],
        )
    };
    let failed = tokio::time::timeout(
        Duration::from_secs(5),
        aether_client::serve::run(
            Arc::new(Recording::default()),
            config,
            futures::stream::empty::<Event>(),
            CancellationToken::new(),
            |bound| assert!(bound.is_none(), "outbound-only serves nothing inbound"),
        ),
    )
    .await
    .expect("the end of the stream is noticed")
    .expect_err("a dropped daemon connection is a failure");
    assert!(failed.to_string().contains("event delivery"), "{failed}");
}
