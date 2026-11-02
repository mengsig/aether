use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

/// Long enough that a blocked read cannot finish while a test looks at it;
/// a test that fails drops its sender, which ends the wait at once.
const HELD: Duration = Duration::from_secs(30);

/// A read that waits for a permit on `gate`, then returns how many reads ran.
fn gated(
    gate: mpsc::Receiver<()>,
) -> (
    Arc<AtomicUsize>,
    impl Fn() -> Result<usize, ErrorReply> + Send + Sync + 'static,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let gate = std::sync::Mutex::new(gate);
    let read = move || {
        let n = counted.fetch_add(1, Ordering::SeqCst) + 1;
        let permit = gate.lock().expect("gate").recv_timeout(HELD);
        permit.map_err(|_| ErrorReply::internal("the test released no permit"))?;
        Ok(n)
    };
    (calls, read)
}

fn stale<T>(answer: Answer<T>) -> Option<(Timestamp, Arc<T>)> {
    match answer {
        Answer::Stale(last) => last,
        Answer::Fresh(_) => panic!("a blocked read answered fresh"),
    }
}

fn fresh<T: Copy>(answer: Answer<T>) -> T {
    match answer {
        Answer::Fresh(value) => *value,
        Answer::Stale(_) => panic!("a finished read answered stale"),
    }
}

#[tokio::test]
async fn a_blocked_read_is_answered_stale_and_shared_by_the_requests_behind_it() {
    let reads = StatusReads::default();
    let (permits, gate) = mpsc::channel();
    let (calls, read) = gated(gate);
    let read = Arc::new(read);
    let ask = |patience| {
        let read = read.clone();
        reads.answer(patience, move || read())
    };
    let started = Instant::now();
    let (first, second) = tokio::join!(
        ask(Duration::from_millis(100)),
        ask(Duration::from_millis(100))
    );
    assert!(
        stale(first.expect("first")).is_none(),
        "nothing was read yet"
    );
    assert!(stale(second.expect("second")).is_none());
    assert!(started.elapsed() < HELD, "answered while the read was held");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "one read for both requests"
    );

    // The read in flight started before the next request: once it finishes it
    // is not that request's answer, and the read after it is held too.
    let mut finished = reads.finished.subscribe();
    permits.send(()).expect("release the first read");
    // Copied out: a borrow of the count held on would block the next read's bump.
    let first_done = tokio::time::timeout(HELD, finished.wait_for(|n| *n >= 1))
        .await
        .map(|count| count.map(|count| *count));
    assert!(
        matches!(first_done, Ok(Ok(1))),
        "the released read finished"
    );
    let last = stale(ask(Duration::from_millis(100)).await.expect("third"));
    let (_, value) = last.expect("the first read finished");
    assert_eq!(*value, 1, "the stale answer is the read that finished");
    assert!(reads.state.lock().running.is_some(), "a newer read runs");

    // `join!` polls the request first, so it has arrived before any release.
    let release = async {
        for _ in 0..3 {
            permits.send(()).expect("release a read");
        }
    };
    let (fourth, ()) = tokio::join!(ask(HELD), release);
    let value = fresh(fourth.expect("fourth"));
    assert!(value >= 2, "a read newer than the request: {value}");
}

#[tokio::test]
async fn a_read_that_fails_or_panics_is_the_requests_error_and_the_next_read_runs() {
    let reads = StatusReads::<u32>::default();
    let failed = reads
        .answer(HELD, || Err(ErrorReply::internal("the store is gone")))
        .await;
    let error = failed.err().expect("the read's own error");
    assert!(error.message.contains("the store is gone"), "{error:?}");
    let panicked = reads
        .answer(HELD, || panic!("a bug in a status read"))
        .await;
    let error = panicked.err().expect("a panic is an error");
    assert!(error.message.contains("panicked"), "{error:?}");
    assert_eq!(fresh(reads.answer(HELD, || Ok(7)).await.expect("read")), 7);
}

#[tokio::test]
async fn a_stale_answer_keeps_the_last_good_read_not_a_failed_one() {
    let reads = StatusReads::<u32>::default();
    assert_eq!(fresh(reads.answer(HELD, || Ok(1)).await.expect("read")), 1);
    let failed = reads
        .answer(HELD, || Err(ErrorReply::internal("busy")))
        .await;
    assert!(failed.is_err());
    let (permits, gate) = mpsc::channel::<()>();
    let gate = std::sync::Mutex::new(gate);
    let held = reads
        .answer(Duration::from_millis(100), move || {
            let permit = gate.lock().expect("gate").recv_timeout(HELD);
            permit.map_err(|_| ErrorReply::internal("released"))?;
            Ok(2)
        })
        .await;
    let (_, value) = stale(held.expect("held")).expect("a good read is kept");
    assert_eq!(*value, 1);
    drop(permits);
}
