use aether_protocol::remote::{
    AttentionKind, DeviceIdentity, DeviceRole, PushConfig, PushSubscription,
};
use aether_remote::DeviceSession;
use aether_remote::push::testing::UserAgent;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::push::tests::{ENDPOINT, Fixture};

fn device() -> (AuthContext, DeviceId) {
    let id = DeviceId::new();
    let session = DeviceSession {
        identity: DeviceIdentity {
            id,
            name: "test phone".into(),
            role: DeviceRole::Observer,
        },
        closed: CancellationToken::new(),
    };
    (AuthContext::Device(session), id)
}

async fn config(fixture: &Fixture, command: Command, auth: &AuthContext) -> PushConfig {
    match dispatch(&fixture.daemon, command, auth).await {
        Ok(Reply::PushConfig(config)) => config,
        other => panic!("expected a push_config reply, got {other:?}"),
    }
}

async fn refused(fixture: &Fixture, command: Command, auth: &AuthContext) -> ErrorReply {
    dispatch(&fixture.daemon, command, auth)
        .await
        .expect_err("a refusal")
}

fn subscribe(subscription: PushSubscription, kinds: Vec<AttentionKind>) -> Command {
    Command::SubscribePush {
        subscription,
        kinds,
    }
}

#[tokio::test]
async fn a_device_subscribes_and_a_second_subscription_replaces_the_first() {
    let fixture = Fixture::start().await;
    let (phone, id) = device();
    let before = config(&fixture, Command::GetPushConfig, &phone).await;
    assert!(!before.enabled && !before.subscribed);
    assert_eq!(
        before.vapid_public_key, None,
        "no key before push is first enabled"
    );

    fixture.set_push(true);
    let enabled = config(&fixture, Command::GetPushConfig, &phone).await;
    let key = enabled
        .vapid_public_key
        .clone()
        .expect("enabling created the key");
    assert!(enabled.enabled && !enabled.subscribed && !enabled.previews);

    let first = UserAgent::generate();
    let kinds = vec![AttentionKind::Approval, AttentionKind::Question];
    let command = subscribe(first.subscription(ENDPOINT), kinds.clone());
    let subscribed = config(&fixture, command, &phone).await;
    assert!(subscribed.subscribed);
    assert_eq!(subscribed.kinds, kinds);
    assert_eq!(subscribed.vapid_public_key.as_deref(), Some(key.as_str()));

    let second = UserAgent::generate();
    let replaced = config(
        &fixture,
        subscribe(second.subscription(ENDPOINT), vec![]),
        &phone,
    )
    .await;
    assert!(replaced.subscribed && replaced.kinds.is_empty());
    let rows = fixture.sqlite.push_subscriptions().expect("store");
    assert_eq!(rows.len(), 1, "a device has at most one subscription");
    assert_eq!(rows[0].device, id);
    fixture.stop().await;
}

#[tokio::test]
async fn a_test_push_reaches_the_current_subscription_and_unsubscribing_forgets_it() {
    let fixture = Fixture::start().await;
    let (phone, _) = device();
    fixture.set_push(true);
    let (first, second) = (UserAgent::generate(), UserAgent::generate());
    for user_agent in [&first, &second] {
        let command = subscribe(user_agent.subscription(ENDPOINT), vec![]);
        config(&fixture, command, &phone).await;
    }

    let tested = dispatch(&fixture.daemon, Command::TestPush, &phone).await;
    assert_eq!(tested, Ok(Reply::Ok));
    let requests = fixture.transport.requests();
    assert_eq!(requests.len(), 1);
    let payload = second.payload(&requests[0].body);
    assert_eq!(payload.kind, AttentionKind::Test);
    assert_eq!(payload.title, "Aether notifications are working");
    first
        .open(&requests[0].body)
        .expect_err("the replaced subscription cannot read it");

    let removed = config(&fixture, Command::UnsubscribePush, &phone).await;
    assert!(!removed.subscribed && removed.enabled);
    let again = config(&fixture, Command::UnsubscribePush, &phone).await;
    assert!(
        !again.subscribed,
        "unsubscribing with no subscription succeeds"
    );
    assert!(
        fixture
            .sqlite
            .push_subscriptions()
            .expect("store")
            .is_empty()
    );
    fixture.stop().await;
}

#[tokio::test]
async fn push_commands_refuse_what_the_contract_refuses() {
    let fixture = Fixture::start().await;
    let (phone, _) = device();
    let user_agent = UserAgent::generate();
    let good = || subscribe(user_agent.subscription(ENDPOINT), vec![]);

    let disabled = refused(&fixture, good(), &phone).await;
    assert_eq!(disabled.code, ErrorCode::Unavailable, "{disabled:?}");
    fixture.set_push(true);

    let no_subscription = refused(&fixture, Command::TestPush, &phone).await;
    assert_eq!(no_subscription.code, ErrorCode::InvalidArgument);

    for endpoint in [
        "http://fcm.googleapis.com/fcm/send/abc",
        "https://user:secret@fcm.googleapis.com/fcm/send/abc",
        "https://127.0.0.1/push",
        "https://169.254.169.254/latest/meta-data",
        "https://internal.example.com/push",
        "https://fcm.googleapis.com:8443/fcm/send/abc",
    ] {
        let error = refused(
            &fixture,
            subscribe(user_agent.subscription(endpoint), vec![]),
            &phone,
        )
        .await;
        assert_eq!(
            error.code,
            ErrorCode::InvalidArgument,
            "{endpoint}: {error:?}"
        );
    }
    let mut bad_keys = user_agent.subscription(ENDPOINT);
    bad_keys.auth = "c2hvcnQ".into();
    let error = refused(&fixture, subscribe(bad_keys, vec![]), &phone).await;
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert!(
        fixture
            .sqlite
            .push_subscriptions()
            .expect("store")
            .is_empty()
    );

    for command in [good(), Command::UnsubscribePush, Command::TestPush] {
        let error = refused(&fixture, command, &AuthContext::Local).await;
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{error:?}");
    }
    let host = config(&fixture, Command::GetPushConfig, &AuthContext::Local).await;
    assert!(
        host.enabled && !host.subscribed,
        "the host reads the config and owns no subscription"
    );
    fixture.stop().await;
}

#[tokio::test]
async fn a_test_push_reports_the_services_answer_and_drops_a_dead_subscription() {
    let fixture = Fixture::start().await;
    let (phone, id) = device();
    fixture.set_push(true);
    let user_agent = UserAgent::generate();
    config(
        &fixture,
        subscribe(user_agent.subscription(ENDPOINT), vec![]),
        &phone,
    )
    .await;

    fixture.transport.answer(503);
    let busy = refused(&fixture, Command::TestPush, &phone).await;
    assert_eq!(busy.code, ErrorCode::Unavailable);
    assert!(busy.message.contains("503"), "{busy:?}");
    assert_eq!(
        fixture.transport.sent(),
        1,
        "a test is not retried behind the caller's back"
    );
    assert!(
        fixture
            .sqlite
            .push_subscription(id)
            .expect("store")
            .is_some()
    );

    fixture.transport.answer(410);
    let gone = refused(&fixture, Command::TestPush, &phone).await;
    assert_eq!(gone.code, ErrorCode::Unavailable);
    assert!(
        fixture
            .sqlite
            .push_subscription(id)
            .expect("store")
            .is_none()
    );
    let after = config(&fixture, Command::GetPushConfig, &phone).await;
    assert!(!after.subscribed);
    fixture.stop().await;
}

#[tokio::test]
async fn a_daemon_assembled_without_push_says_so() {
    let mut fixture = Fixture::start().await;
    fixture.daemon.push = None;
    let (phone, _) = device();
    for command in [
        Command::GetPushConfig,
        Command::UnsubscribePush,
        Command::TestPush,
    ] {
        let error = refused(&fixture, command, &phone).await;
        assert_eq!(error.code, ErrorCode::Unavailable);
    }
    fixture.stop().await;
}
