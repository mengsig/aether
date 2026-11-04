//! The lease on paused time: when it reads, when it ends, and that nothing is relayed after.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use futures::{SinkExt as _, StreamExt as _};
use tokio::time::Instant;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_util::sync::CancellationToken;

use super::{Blocked, LEASE_AGE, Lease, REFRESH_EVERY};
use crate::Tuning;
use crate::mount::tests::{Status, StatusHost};
use crate::mount::tunnel::{Ends, relay_frames};

/// A lease whose guard reads `host`, stopped when the test ends.
fn guarded(host: &Arc<StatusHost>) -> (Arc<Lease>, tokio_util::sync::DropGuard) {
    let lease = Arc::new(Lease::default());
    let stop = CancellationToken::new();
    tokio::spawn(lease.clone().guard(host.clone(), stop.clone()));
    (lease, stop.drop_guard())
}

fn reads(host: &StatusHost) -> usize {
    host.reads.load(Ordering::SeqCst)
}

#[tokio::test(start_paused = true)]
async fn an_admission_reads_first_and_an_open_ticket_reads_every_two_seconds() {
    let host = StatusHost::new(Status::Private);
    let (lease, _stop) = guarded(&host);
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert_eq!(reads(&host), 0, "nothing open, nothing read");
    let ticket = lease.admit().await.expect("a private status admits");
    assert_eq!(reads(&host), 1);
    tokio::time::sleep(REFRESH_EVERY * 5 + Duration::from_millis(10)).await;
    assert_eq!(reads(&host), 6, "one read every two seconds while held");
    assert!(ticket.live());
    drop(ticket);
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert_eq!(reads(&host), 6, "no holder, no reads");
}

#[tokio::test(start_paused = true)]
async fn an_unheld_lease_expires_on_its_own_timer_and_the_next_admission_reads_again() {
    let host = StatusHost::new(Status::Private);
    let (lease, _stop) = guarded(&host);
    drop(lease.admit().await.expect("admitted"));
    let ticket = lease.admit().await.expect("still fresh: no second read");
    assert_eq!(reads(&host), 1);
    drop(ticket);
    tokio::time::sleep(LEASE_AGE + Duration::from_millis(1)).await;
    assert!(
        lease.state.lock().expires.is_none(),
        "the timer ended the lease"
    );
    lease.admit().await.expect("read again");
    assert_eq!(reads(&host), 2);
}

#[tokio::test(start_paused = true)]
async fn a_funnel_ends_the_lease_at_the_next_read_and_holds_admissions_off_without_rereading() {
    let host = StatusHost::new(Status::Private);
    let (lease, _stop) = guarded(&host);
    let ticket = lease.admit().await.expect("admitted");
    host.set(Status::Funnel);
    let started = Instant::now();
    ticket.ended().await;
    assert!(
        started.elapsed() <= REFRESH_EVERY,
        "{:?}",
        started.elapsed()
    );
    assert!(!ticket.live());
    let seen = reads(&host);
    for _ in 0..10 {
        assert_eq!(lease.admit().await.err(), Some(Blocked::Funnel));
    }
    assert_eq!(
        reads(&host),
        seen,
        "a fresh refusal answers without reading"
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        lease.admit().await.err(),
        Some(Blocked::Funnel),
        "read again: still a Funnel"
    );
    assert_eq!(reads(&host), seen + 1);
}

#[tokio::test(start_paused = true)]
async fn a_failing_or_hanging_read_ends_the_lease_within_its_age() {
    for broken in [Status::Failing, Status::Hanging] {
        let host = StatusHost::new(Status::Private);
        let (lease, _stop) = guarded(&host);
        let admitted = Instant::now();
        let ticket = lease.admit().await.expect("admitted");
        host.set(broken);
        ticket.ended().await;
        assert!(
            admitted.elapsed() <= LEASE_AGE,
            "{broken:?}: {:?}",
            admitted.elapsed()
        );
        let refused = lease.admit().await.err();
        assert_eq!(refused, Some(Blocked::StatusUnknown), "{broken:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn an_admission_waits_at_most_the_lease_age_for_a_read() {
    let host = StatusHost::new(Status::Hanging);
    let (lease, _stop) = guarded(&host);
    let started = Instant::now();
    assert_eq!(lease.admit().await.err(), Some(Blocked::StatusUnknown));
    assert!(started.elapsed() <= LEASE_AGE, "{:?}", started.elapsed());
}

#[tokio::test(start_paused = true)]
async fn a_stopped_guard_ends_the_lease() {
    let host = StatusHost::new(Status::Private);
    let lease = Arc::new(Lease::default());
    let stop = CancellationToken::new();
    let guard = tokio::spawn(lease.clone().guard(host.clone(), stop.clone()));
    let ticket = lease.admit().await.expect("admitted");
    stop.cancel();
    guard.await.expect("the guard ends");
    assert!(!ticket.live());
    assert!(ticket.epoch.is_cancelled());
}

/// No guard runs here, so no timer ends the lease: only the per-frame check stands between
/// an expired lease and the target.
#[tokio::test(start_paused = true)]
async fn no_frame_reaches_the_target_once_the_lease_is_over_even_before_its_timer_fires() {
    let lease = Arc::new(Lease::default());
    lease.renew(Instant::now());
    let ticket = lease.try_admit().expect("a live lease");
    let (client_end, relay_client) = tokio::io::duplex(64 * 1024);
    let (relay_target, target_end) = tokio::io::duplex(64 * 1024);
    let ends = Ends {
        ticket,
        remounted: CancellationToken::new(),
        stop: CancellationToken::new(),
    };
    let relay = tokio::spawn(async move {
        relay_frames(relay_client, relay_target, &ends, Tuning::default()).await;
    });
    let mut client = WebSocketStream::from_raw_socket(client_end, Role::Client, None).await;
    let mut target = WebSocketStream::from_raw_socket(target_end, Role::Server, None).await;

    client.send(Message::text("early")).await.expect("send");
    let early = target.next().await.expect("a frame").expect("read");
    assert_eq!(early, Message::text("early"));

    tokio::time::advance(LEASE_AGE).await;
    client.send(Message::text("late")).await.expect("send");
    let closed = client.next().await.expect("a close").expect("read");
    let Message::Close(Some(frame)) = closed else {
        panic!("expected a close, got {closed:?}");
    };
    assert_eq!(u16::from(frame.code), 1008);
    relay.await.expect("the relay ends");
    let after: Vec<_> = target.collect().await;
    assert!(
        after.iter().all(|read| read.is_err()),
        "nothing, not even a close frame, reached the target: {after:?}"
    );
}
