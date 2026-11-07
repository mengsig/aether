//! `status` within a deadline: a section the store does not answer in time
//! is named unavailable, the others are reported, and a stalled read is
//! never started twice.
use super::*;
use aether_protocol::UnavailableSection;
use std::sync::mpsc;
use std::time::Instant;

fn status_within(runtime: &Arc<Runtime>, patience: Duration) -> aether_runtime::RuntimeStatus {
    let started = Instant::now();
    let status = runtime.status_within(patience).expect("status");
    assert!(
        started.elapsed() < patience + Duration::from_secs(5),
        "status kept to its deadline"
    );
    status
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stalled_section_is_named_unavailable_and_never_read_twice() {
    let inner: Arc<dyn Store> = Arc::new(SqliteStore::open_in_memory().expect("store"));
    let store = Arc::new(CommitsMidPass::new(inner, Hook::FleetUsage));
    let h = harness_full(
        vec![],
        |_| {},
        |_| {},
        None,
        None,
        Some(store.clone()),
        |_| {},
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(idle_spec(), Actor::user())
        .expect("agent");
    h.wait_until(agent.id, |s| s.kind() == AgentStatusKind::Waiting)
        .await;
    let (release, stalled) = mpsc::channel::<()>();
    store.arm(Box::new(move |_| {
        // Bounded, so a broken test fails instead of hanging.
        stalled
            .recv_timeout(Duration::from_secs(30))
            .expect("the test releases the stalled read");
        Ok(())
    }));
    let usage_total = vec![UnavailableSection {
        section: "usage_total".into(),
        reason: "timed out".into(),
    }];

    let status = status_within(&h.runtime, Duration::from_secs(3));
    assert!(
        store.fired(),
        "the usage read reached the store and stalled"
    );
    assert_eq!(status.unavailable, usage_total);
    assert_eq!(status.usage_total, Default::default());
    assert!(
        status.agents_live >= 1,
        "the sections that answered are reported"
    );

    // The armed stall is spent: only the in-flight gate keeps this read off the store.
    let again = status_within(&h.runtime, Duration::from_secs(3));
    assert_eq!(
        again.unavailable, usage_total,
        "the stalled read is not doubled"
    );

    release.send(()).expect("the stalled read is still waiting");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = status_within(&h.runtime, Duration::from_secs(2));
        if status.unavailable.is_empty() {
            break;
        }
        assert!(Instant::now() < deadline, "the released read finished");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
