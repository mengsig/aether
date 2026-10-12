//! A device that renews its subscription while a send to the old one is
//! still with the push service: the answer is about the old one only.

use std::sync::Arc;
use std::time::Duration;

use aether_protocol::remote::DeviceId;

use super::harness::{ENDPOINT, Harness, Held, SETTLED, failed, settings};
use crate::push::testing::{RecordingTransport, UserAgent};
use crate::push::{PushError, PushStore};

const RENEWED: &str = "https://updates.push.services.mozilla.com/wpush/v2/renewed";

/// A device subscribed at [`ENDPOINT`], and the browser key it will renew with.
fn subscribed(held: &Arc<Held>) -> (Harness, DeviceId, UserAgent) {
    let recorder = Arc::new(RecordingTransport::default());
    let harness = Harness::with_transport(false, recorder, held.clone());
    harness
        .notifier
        .apply_settings(&settings(true))
        .expect("enabling");
    let device = DeviceId::new();
    let old = UserAgent::generate().subscription(ENDPOINT);
    harness
        .notifier
        .subscribe(device, old, Vec::new())
        .expect("subscribing");
    (harness, device, UserAgent::generate())
}

#[tokio::test(start_paused = true)]
async fn a_410_for_a_subscription_renewed_mid_send_spares_the_new_one_and_its_queue() {
    let held = Held::new();
    let (harness, device, renewed) = subscribed(&held);
    let first = harness.emit(harness.agent("first"), failed());
    let second = harness.emit(harness.agent("second"), failed());
    tokio::time::sleep(SETTLED).await;
    assert_eq!(held.endpoints(), [ENDPOINT], "one send is with the service");

    // The browser renews while that request is still unanswered.
    harness
        .notifier
        .subscribe(device, renewed.subscription(RENEWED), Vec::new())
        .expect("renewing");
    for status in [410, 201, 201] {
        held.answer(status);
    }
    tokio::time::sleep(SETTLED).await;

    let stored = harness.store.subscription(device).expect("reading");
    let stored = stored.expect("the 410 was about the old subscription");
    assert_eq!(stored.subscription, renewed.subscription(RENEWED));
    assert_eq!(
        held.endpoints(),
        [ENDPOINT, RENEWED, RENEWED],
        "both notices went to the new subscription, the refused one again"
    );
    let asked = held.asked.lock();
    let mut seqs: Vec<u64> = asked[1..]
        .iter()
        .map(|request| renewed.payload(&request.body).seq)
        .collect();
    seqs.sort_unstable();
    assert_eq!(seqs, [first, second]);
}

#[tokio::test(start_paused = true)]
async fn a_test_push_refused_for_a_subscription_renewed_meanwhile_removes_nothing() {
    let held = Held::new();
    let (harness, device, renewed) = subscribed(&held);
    let waiting = harness.emit(harness.agent("root"), failed());
    let testing = tokio::spawn({
        let notifier = harness.notifier.clone();
        async move { notifier.test(device).await }
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(held.endpoints(), [ENDPOINT], "the test is with the service");

    harness
        .notifier
        .subscribe(device, renewed.subscription(RENEWED), Vec::new())
        .expect("renewing");
    held.answer(410);
    let refused = testing.await.expect("joined").expect_err("a 410");
    assert!(
        matches!(&refused, PushError::Service(why) if why.contains("410") && why.contains("replaced")),
        "{refused}"
    );
    let stored = harness.store.subscription(device).expect("reading");
    assert_eq!(
        stored.map(|stored| stored.subscription),
        Some(renewed.subscription(RENEWED))
    );
    assert_eq!(harness.notifier.queued(), 1, "what waited still does");

    held.answer(201);
    tokio::time::sleep(SETTLED).await;
    assert_eq!(held.endpoints(), [ENDPOINT, RENEWED]);
    let delivered = renewed.payload(&held.asked.lock()[1].body);
    assert_eq!(delivered.seq, waiting);
}

#[tokio::test(start_paused = true)]
async fn a_410_still_removes_the_subscription_it_was_about() {
    let held = Held::new();
    let (harness, device, _renewed) = subscribed(&held);
    harness.emit(harness.agent("root"), failed());
    tokio::time::sleep(SETTLED).await;
    assert_eq!(held.endpoints(), [ENDPOINT]);

    held.answer(410);
    tokio::time::sleep(SETTLED).await;
    assert_eq!(harness.store.subscription(device).expect("reading"), None);
    assert_eq!(harness.notifier.queued(), 0);
}
