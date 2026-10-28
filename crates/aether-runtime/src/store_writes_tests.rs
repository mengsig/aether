use super::*;

fn note() -> Commit {
    Commit::new().event(EventDraft::new(
        Actor::System,
        EventKind::Custom {
            name: "test.note".into(),
            payload: serde_json::json!({}),
        },
    ))
}

/// How long a thread outside the runtime keeps the commit gate.
const GATE_HELD: std::time::Duration = std::time::Duration::from_secs(1);

/// The daemon's setting, on one worker: a write waiting for the gate hands
/// the worker on, so a task queued behind it runs while the gate is held.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn a_write_waiting_for_the_gate_stalls_no_other_task() {
    let (runtime, _dir) = test_runtime(|config| config.store_writes_off_worker = true).await;
    let before = runtime.store.last_seq().unwrap();
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let ahead = {
        let runtime = runtime.clone();
        std::thread::spawn(move || {
            let hold = runtime.hold();
            held_tx.send(()).expect("signal");
            std::thread::sleep(GATE_HELD);
            runtime.apply_held(hold, note())
        })
    };
    held_rx.recv().expect("the gate is held");
    let behind = {
        let runtime = runtime.clone();
        tokio::spawn(async move { runtime.apply(note()) })
    };
    // Off the runtime: this thread is not its worker.
    std::thread::sleep(std::time::Duration::from_millis(100));
    let queued = Instant::now();
    let waited = tokio::spawn(async move { queued.elapsed() })
        .await
        .expect("join");
    assert!(waited < GATE_HELD / 2, "the queued task waited {waited:?}");
    ahead.join().expect("join").expect("first write");
    behind.await.expect("join").expect("second write");
    assert_eq!(runtime.store.last_seq().unwrap(), before + 2);
}

/// `block_in_place` panics inside a `LocalSet`, even on a multi-threaded
/// runtime, so a runtime not told it may hand the worker off writes inline.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_inside_a_local_set_is_not_handed_off() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    assert!(!runtime.config.store_writes_off_worker);
    let before = runtime.store.last_seq().unwrap();
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async { runtime.apply(note()) })
        .await
        .expect("a write inside a LocalSet");
    assert_eq!(runtime.store.last_seq().unwrap(), before + 1);
}
