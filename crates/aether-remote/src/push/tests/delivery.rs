//! What the delivery worker does with the bus's triggers and with a push
//! service's answers. Time is paused, so every wait is exact.

use std::sync::Arc;
use std::time::Duration;

use aether_core::event::{Actor, EventKind};
use aether_core::id::ApprovalId;
use aether_core::tool::ApprovalDecision;
use aether_protocol::remote::AttentionKind;
use tokio::time::Instant;

use super::harness::{Harness, SECRET, SETTLED, by_device, failed, settings};
use crate::push::outbox::{BACKOFF_BASE, BACKOFF_MAX, CAPACITY, LIFETIME, QUIET, SETTLE};
use crate::push::testing::RecordingTransport;
use crate::push::transport::{PushResponse, TransportError};
use crate::push::{PushRequest, PushStore, Transport};

fn retry_after(status: u16, seconds: u64) -> Result<PushResponse, TransportError> {
    Ok(PushResponse {
        status,
        retry_after: Some(Duration::from_secs(seconds)),
    })
}

#[tokio::test(start_paused = true)]
async fn a_burst_for_one_agent_is_one_notification_then_one_more() {
    let harness = Harness::start(false);
    let (_device, agent) = harness.subscribe();
    let root = harness.agent("root");
    let start = Instant::now();

    let mut last = 0;
    for _ in 0..6 {
        last = harness.emit(root, failed());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(SETTLED).await;
    let received = harness.received(&agent);
    assert_eq!(
        received.len(),
        1,
        "six failures in a burst are one notification"
    );
    assert_eq!(received[0].seq, last, "and it is about the newest");
    assert_eq!(harness.transport.times()[0], start + SETTLE);

    // More inside the quiet window wait for it to end, and collapse again.
    harness.emit(root, failed());
    let newest = harness.emit(root, failed());
    tokio::time::sleep(QUIET).await;
    let received = harness.received(&agent);
    assert_eq!(received.len(), 2);
    assert_eq!(received[1].seq, newest);
    assert_eq!(harness.transport.times()[1], start + SETTLE + QUIET);

    let requests = harness.transport.requests();
    assert_eq!(
        requests[0].topic, requests[1].topic,
        "the push service replaces by topic too"
    );
}

#[tokio::test(start_paused = true)]
async fn agents_and_kinds_of_notice_do_not_swallow_each_other() {
    let harness = Harness::start(false);
    let (_device, agent) = harness.subscribe();
    let (first, second) = (harness.agent("first"), harness.agent("second"));

    harness.emit(first, failed());
    harness.emit(second, failed());
    let (approval, _) = harness.ask_approval(first);
    tokio::time::sleep(SETTLED * 2).await;

    let received = harness.received(&agent);
    assert_eq!(received.len(), 3);
    let failures: Vec<_> = received
        .iter()
        .filter(|p| p.kind == AttentionKind::AgentFailed)
        .collect();
    assert_eq!(failures.len(), 2);
    assert_ne!(failures[0].agent, failures[1].agent);
    assert!(received.iter().any(|p| p.approval == Some(approval)));
    let topics: std::collections::HashSet<_> = harness
        .transport
        .requests()
        .into_iter()
        .map(|r| r.topic)
        .collect();
    assert_eq!(topics.len(), 3);
}

#[tokio::test(start_paused = true)]
async fn an_approval_decided_before_it_was_sent_is_not_announced() {
    let harness = Harness::start(false);
    let (_device, agent) = harness.subscribe();
    let root = harness.agent("root");
    let (approval, _) = harness.ask_approval(root);
    let resolved = EventKind::ApprovalResolved {
        approval,
        decision: ApprovalDecision::Approved,
        by: Actor::user(),
        note: None,
    };
    harness.emit(root, resolved);
    tokio::time::sleep(SETTLED).await;
    assert_eq!(harness.received(&agent), [], "nothing waits any more");

    // A rule that answered at once never notifies in the first place.
    harness.emit(root, super::harness::approval_requested(ApprovalId::new()));
    tokio::time::sleep(SETTLED).await;
    assert_eq!(harness.transport.sent(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_device_is_not_told_about_what_it_just_did_itself() {
    let harness = Harness::start(false);
    let (phone, phone_agent) = harness.subscribe();
    let (_tablet, tablet_agent) = harness.subscribe();
    let root = harness.agent("root");

    let event = harness.event(by_device(phone), root, failed());
    harness.notifier.observe(&event).expect("observing");
    tokio::time::sleep(SETTLED).await;
    assert_eq!(
        harness.received(&phone_agent),
        [],
        "the acting device stays quiet"
    );
    assert_eq!(
        harness.received(&tablet_agent).len(),
        1,
        "the other one is told"
    );
}

#[tokio::test(start_paused = true)]
async fn nothing_is_sent_while_notifications_are_disabled() {
    let harness = Harness::start(false);
    let (device, agent) = harness.subscribe();
    let root = harness.agent("root");

    harness.emit(root, failed());
    assert_eq!(harness.notifier.queued(), 1);
    harness
        .notifier
        .apply_settings(&settings(false))
        .expect("disabling");
    assert_eq!(
        harness.notifier.queued(),
        0,
        "disabling drops what was waiting"
    );
    harness.emit(root, failed());
    harness.ask_approval(root);
    tokio::time::sleep(LIFETIME).await;
    assert_eq!(harness.transport.sent(), 0);
    assert!(
        harness
            .store
            .subscription(device)
            .expect("reading")
            .is_some(),
        "the subscription stays"
    );

    harness
        .notifier
        .apply_settings(&settings(true))
        .expect("enabling");
    harness.emit(root, failed());
    tokio::time::sleep(SETTLED).await;
    assert_eq!(harness.received(&agent).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn previews_are_sent_only_when_allowed() {
    for previews in [false, true] {
        let harness = Harness::start(previews);
        let (_device, agent) = harness.subscribe();
        let root = harness.agent("root");
        harness.emit(
            root,
            EventKind::QuestionAsked {
                question: SECRET.into(),
            },
        );
        tokio::time::sleep(SETTLED).await;

        let request = &harness.transport.requests()[0];
        let plaintext = String::from_utf8(agent.open(&request.body).expect("ours")).expect("utf-8");
        assert_eq!(
            plaintext.contains("secret-project"),
            previews,
            "{plaintext}"
        );
        let payload = &harness.received(&agent)[0];
        assert_eq!(payload.preview.is_some(), previews);
        assert_eq!(payload.title, "An agent has a question");
    }
}

#[tokio::test(start_paused = true)]
async fn a_gone_subscription_is_deleted_and_its_queue_dropped() {
    for status in [404, 410] {
        let harness = Harness::start(false);
        let (gone, _) = harness.subscribe();
        harness.transport.answer(status);
        let (first, second) = (harness.agent("first"), harness.agent("second"));
        harness.emit(first, failed());
        harness.emit(second, failed());
        tokio::time::sleep(SETTLED).await;

        assert_eq!(
            harness.transport.sent(),
            1,
            "{status}: the second notice is not tried"
        );
        assert_eq!(
            harness.store.subscriptions().expect("listing"),
            [],
            "{status}"
        );
        assert_eq!(harness.notifier.queued(), 0);
        assert!(
            !harness
                .notifier
                .config(Some(gone))
                .expect("config")
                .subscribed
        );

        harness.emit(first, failed());
        tokio::time::sleep(LIFETIME).await;
        assert_eq!(
            harness.transport.sent(),
            1,
            "{status}: nothing more is ever sent"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn failures_are_retried_with_bounded_jittered_backoff() {
    let harness = Harness::start(false);
    let (device, agent) = harness.subscribe();
    harness.transport.answer(503);
    harness.transport.answer(500);
    harness
        .transport
        .answer_with(Err(TransportError::Unreachable("connection reset".into())));
    harness.transport.answer(429);
    let seq = harness.emit(harness.agent("root"), failed());
    tokio::time::sleep(Duration::from_secs(600)).await;

    let times = harness.transport.times();
    assert_eq!(times.len(), 5, "four failures, then it is delivered");
    for (failures, pair) in times.windows(2).enumerate() {
        let step = BACKOFF_BASE * 2u32.pow(failures as u32);
        let waited = pair[1] - pair[0];
        assert!(
            step / 2 <= waited && waited <= step,
            "retry {failures}: {waited:?} of {step:?}"
        );
    }
    let received = harness.received(&agent);
    assert!(
        received.iter().all(|payload| payload.seq == seq),
        "the same notice each time"
    );
    assert!(
        harness
            .store
            .subscription(device)
            .expect("reading")
            .is_some(),
        "a 5xx keeps the subscription"
    );
    assert_eq!(
        harness.notifier.queued(),
        1,
        "only the quiet window remains"
    );
}

#[tokio::test(start_paused = true)]
async fn retry_after_is_honoured_for_the_whole_device() {
    let harness = Harness::start(false);
    let (_device, _agent) = harness.subscribe();
    harness.transport.answer_with(retry_after(429, 300));
    harness.emit(harness.agent("first"), failed());
    tokio::time::sleep(SETTLED).await;
    assert_eq!(harness.transport.sent(), 1);

    // A different agent's notice goes to the same push service: it waits too.
    harness.emit(harness.agent("second"), failed());
    tokio::time::sleep(Duration::from_secs(298)).await;
    assert_eq!(
        harness.transport.sent(),
        1,
        "nothing before the service's own deadline"
    );
    tokio::time::sleep(Duration::from_secs(3)).await;

    let times = harness.transport.times();
    assert_eq!(times.len(), 3, "both go once the hold ends");
    assert_eq!(times[1] - times[0], Duration::from_secs(300));
}

#[tokio::test(start_paused = true)]
async fn a_dead_push_service_is_given_up_on_within_the_hour() {
    let harness = Harness::start(false);
    let (device, _agent) = harness.subscribe();
    for _ in 0..64 {
        harness.transport.answer(503);
    }
    let start = Instant::now();
    harness.emit(harness.agent("root"), failed());
    tokio::time::sleep(LIFETIME * 3).await;

    let times = harness.transport.times();
    assert!(
        (7..=14).contains(&times.len()),
        "a bounded number of tries: {}",
        times.len()
    );
    assert!(
        times.iter().all(|at| *at < start + LIFETIME),
        "none after the hour"
    );
    for pair in times.windows(2) {
        assert!(pair[1] - pair[0] <= BACKOFF_MAX, "no gap beyond the cap");
    }
    assert_eq!(harness.notifier.queued(), 0, "the notice was given up");
    assert!(
        harness
            .store
            .subscription(device)
            .expect("reading")
            .is_some()
    );
}

#[tokio::test(start_paused = true)]
async fn a_rejected_message_is_dropped_without_retry() {
    for status in [400, 401, 403, 413] {
        let harness = Harness::start(false);
        let (device, _agent) = harness.subscribe();
        harness.transport.answer(status);
        harness.emit(harness.agent("root"), failed());
        tokio::time::sleep(LIFETIME).await;
        assert_eq!(harness.transport.sent(), 1, "{status}");
        assert!(
            harness
                .store
                .subscription(device)
                .expect("reading")
                .is_some(),
            "{status}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn test_push_reports_what_the_service_said() {
    let harness = Harness::start(true);
    let (device, agent) = harness.subscribe();
    harness.notifier.test(device).await.expect("accepted");
    let payload = &harness.received(&agent)[0];
    assert_eq!((payload.kind, payload.seq), (AttentionKind::Test, 0));
    assert_eq!(payload.preview, None);

    harness.transport.answer(503);
    let refused = harness.notifier.test(device).await.expect_err("a 503");
    assert!(refused.to_string().contains("503"), "{refused}");
    assert_eq!(harness.transport.sent(), 2, "a failed test is not retried");
    tokio::time::sleep(LIFETIME).await;
    assert_eq!(harness.transport.sent(), 2);

    harness.transport.answer(410);
    let gone = harness.notifier.test(device).await.expect_err("a 410");
    assert!(gone.to_string().contains("410"), "{gone}");
    assert_eq!(harness.store.subscriptions().expect("listing"), []);
}

/// A push service that never answers.
#[derive(Debug, Default)]
struct BlackHole {
    posted: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl Transport for BlackHole {
    async fn post(&self, _request: PushRequest) -> Result<PushResponse, TransportError> {
        self.posted
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::future::pending().await
    }
}

#[tokio::test(start_paused = true)]
async fn a_hung_push_service_neither_grows_the_queue_nor_stalls_the_bus() {
    let hole = Arc::new(BlackHole::default());
    let harness =
        Harness::with_transport(false, Arc::new(RecordingTransport::default()), hole.clone());
    harness
        .notifier
        .apply_settings(&settings(true))
        .expect("enabling");
    let devices: Vec<_> = (0..3).map(|_| harness.subscribe()).collect();

    for round in 0..2_000 {
        // `emit` returns at once: observing never waits on the network.
        harness.emit(harness.agent("worker"), failed());
        if round % 100 == 0 {
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        assert!(harness.notifier.queued() <= CAPACITY, "round {round}");
    }
    tokio::time::sleep(LIFETIME * 2).await;
    let posted = hole.posted.load(std::sync::atomic::Ordering::Relaxed);
    assert!(posted >= devices.len(), "every device was tried: {posted}");
    assert!(
        posted <= devices.len() * 40,
        "and retried only with backoff: {posted}"
    );
    assert_eq!(
        harness.notifier.queued(),
        0,
        "what could not be sent expired"
    );
}
