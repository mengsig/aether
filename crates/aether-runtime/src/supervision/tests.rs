use super::*;
use parking_lot::Mutex;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test(start_paused = true)]
async fn panics_and_unexpected_exits_restart_with_backoff() {
    let shutdown = CancellationToken::new();
    let attempts = Arc::new(AtomicUsize::new(0));
    let notices = Arc::new(Mutex::new(Vec::new()));
    let count = attempts.clone();
    let log = notices.clone();
    let stop = shutdown.clone();
    let task = tokio::spawn(restart(
        "watcher",
        shutdown.clone(),
        move |s| log.lock().push(s),
        move || {
            let attempt = count.fetch_add(1, Ordering::SeqCst);
            let stop = stop.clone();
            async move {
                match attempt {
                    0 => panic!("bad external response"),
                    1 => (),
                    _ => stop.cancelled().await,
                }
            }
        },
    ));
    tokio::task::yield_now().await;
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::task::yield_now().await;
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
    let notices = notices.lock();
    assert_eq!(notices.len(), 2);
    assert!(notices[0].contains("watcher") && notices[0].contains("aether doctor"));
}

#[tokio::test(start_paused = true)]
async fn cancellation_interrupts_backoff_without_restarting() {
    let stop = CancellationToken::new();
    let attempts = Arc::new(AtomicUsize::new(0));
    let count = attempts.clone();
    let task = tokio::spawn(restart(
        "watcher",
        stop.clone(),
        |_| {},
        move || {
            count.fetch_add(1, Ordering::SeqCst);
            async {}
        },
    ));
    tokio::task::yield_now().await;
    stop.cancel();
    tokio::time::timeout(Duration::from_millis(10), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn stuck_worker_is_dropped_with_visible_notice_on_shutdown() {
    let stop = CancellationToken::new();
    let notices = Arc::new(Mutex::new(Vec::new()));
    let log = notices.clone();
    let task = tokio::spawn(isolate(
        "stuck",
        stop.clone(),
        false,
        move |s| log.lock().push(s),
        std::future::pending(),
    ));
    tokio::task::yield_now().await;
    stop.cancel();
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(5)).await;
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(notices.lock().len(), 1);
    assert!(notices.lock()[0].contains("five seconds"));
}

#[tokio::test]
async fn one_shot_panic_is_visible_but_normal_completion_is_not_an_error() {
    let notices = Mutex::new(Vec::new());
    isolate(
        "discovery",
        CancellationToken::new(),
        true,
        |s| notices.lock().push(s),
        async { panic!("bad server") },
    )
    .await;
    isolate(
        "discovery",
        CancellationToken::new(),
        true,
        |s| notices.lock().push(s),
        async {},
    )
    .await;
    assert_eq!(notices.lock().len(), 1);
    assert!(notices.lock()[0].contains("disabled"));
}
