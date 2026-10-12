//! Every flight taken and more due: the worker waits for a send to land, a
//! change or shutdown, and is not polled in the meantime.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use super::harness::{Harness, Held, failed, settings};
use crate::push::outbox::SETTLE;
use crate::push::testing::RecordingTransport;

/// The sends under way at once; a fifth device is one too many.
const FLIGHTS: usize = 4;
/// Turns of the runtime given to a worker that should have nothing to do.
const IDLE_TURNS: usize = 1_000;
/// A worker that is woken, looks and waits again is polled about this often.
const POLLS_PER_WAKE: usize = 4;

/// Counts how often the runtime polls the future inside.
struct Polled<F> {
    inner: Pin<Box<F>>,
    polls: Arc<AtomicUsize>,
}

impl<F: Future> Future for Polled<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        self.polls.fetch_add(1, Ordering::Relaxed);
        self.inner.as_mut().poll(cx)
    }
}

/// Let every task that can run do so, without moving the clock.
async fn turns(count: usize) {
    for _ in 0..count {
        tokio::task::yield_now().await;
    }
}

/// How often the worker is polled over [`IDLE_TURNS`] turns of the runtime.
async fn polls_while_idle(polls: &AtomicUsize) -> usize {
    let before = polls.load(Ordering::Relaxed);
    turns(IDLE_TURNS).await;
    polls.load(Ordering::Relaxed) - before
}

#[tokio::test(start_paused = true)]
async fn a_full_flight_set_parks_the_worker_until_something_changes() {
    let held = Held::new();
    let recorder = Arc::new(RecordingTransport::default());
    let harness = Harness::without_worker(false, recorder, held.clone());
    harness
        .notifier
        .apply_settings(&settings(true))
        .expect("enabling");
    let polls = Arc::new(AtomicUsize::new(0));
    let (notifier, stop) = (harness.notifier.clone(), harness.shutdown());
    let worker = tokio::spawn(Polled {
        inner: Box::pin(async move { notifier.run(stop).await }),
        polls: polls.clone(),
    });
    for _ in 0..=FLIGHTS {
        harness.subscribe();
    }
    harness.emit(harness.agent("first"), failed());
    tokio::time::advance(SETTLE).await;
    turns(64).await;
    assert_eq!(held.endpoints().len(), FLIGHTS, "every flight is taken");
    assert_eq!(
        harness.notifier.queued(),
        FLIGHTS + 1,
        "and one more is due"
    );

    let spun = polls_while_idle(&polls).await;
    assert!(spun <= POLLS_PER_WAKE, "polled {spun} times with no room");

    // A change wakes it once: it finds no room and waits again.
    harness.emit(harness.agent("second"), failed());
    let woken = polls_while_idle(&polls).await;
    assert!(
        woken <= POLLS_PER_WAKE,
        "polled {woken} times for one change"
    );
    assert_eq!(held.endpoints().len(), FLIGHTS);

    // A send that lands makes room for the one that was due all along.
    held.answer(201);
    let landed = polls_while_idle(&polls).await;
    assert!(
        landed <= POLLS_PER_WAKE,
        "polled {landed} times for a landing"
    );
    assert_eq!(
        held.endpoints().len(),
        FLIGHTS + 1,
        "the fifth device is sent to"
    );

    harness.shutdown().cancel();
    worker
        .await
        .expect("shutdown ends a worker that is waiting");
}
