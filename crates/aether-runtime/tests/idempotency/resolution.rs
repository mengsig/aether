use super::*;
use aether_core::idempotency::{SubmissionHorizon, UnprovenAbsence};
use aether_runtime::submission::SubmissionStanding;

/// What a client was last told, kept with a key when it is first sent.
fn horizon(fx: &Fixture) -> SubmissionHorizon {
    fx.runtime.submission_horizon().expect("horizon")
}

fn standing(
    fx: &Fixture,
    submission: &Submission,
    sent_under: Option<&SubmissionHorizon>,
) -> SubmissionStanding {
    fx.runtime
        .resolve_submission(submission, sent_under)
        .expect("resolve")
        .standing
}

fn send(fx: &Fixture, to: AgentId, submission: &Submission) -> MessageId {
    fx.runtime
        .send_message_keyed(to, text("ship it"), Actor::user(), Some(submission.clone()))
        .expect("delivery")
}

fn deliveries(fx: &Fixture, agent: AgentId) -> usize {
    fx.store.inbox(agent, 0).expect("inbox").len()
}

/// A store whose receipt cap a test reaches in a handful of sends.
async fn capped(max_submissions: usize) -> Fixture {
    let store = SqliteStore::open_in_memory()
        .expect("store")
        .with_submission_limits(max_submissions, 1);
    over(Arc::new(store)).await
}

#[tokio::test]
async fn a_recorded_submission_resolves_as_executed_and_replays_its_receipt() {
    let fx = fixture().await;
    let agent = an_agent(&fx).await;
    let sent_under = horizon(&fx);
    let submission = message_submission("lost-ack", &agent.to_string(), "ship it");
    let first = send(&fx, agent, &submission);

    assert_eq!(
        standing(&fx, &submission, Some(&sent_under)),
        SubmissionStanding::Recorded
    );
    assert_eq!(
        standing(&fx, &submission, None),
        SubmissionStanding::Recorded,
        "a held receipt needs no horizon to be believed"
    );
    let replayed = fx
        .runtime
        .send_message_keyed(
            agent,
            text("ship it"),
            Actor::user(),
            Some(submission.replay_only()),
        )
        .expect("replay");
    assert_eq!(replayed, first, "the receipt a duplicate would be given");
    assert_eq!(deliveries(&fx, agent), 1, "resolving delivered nothing");
}

#[tokio::test]
async fn an_unrecorded_submission_resolves_as_never_executed_and_the_resend_runs_once() {
    let fx = fixture().await;
    let agent = an_agent(&fx).await;
    let sent_under = horizon(&fx);
    let submission = message_submission("never-arrived", &agent.to_string(), "ship it");

    assert_eq!(
        standing(&fx, &submission, Some(&sent_under)),
        SubmissionStanding::NeverExecuted
    );
    assert_eq!(deliveries(&fx, agent), 0, "resolving executes nothing");
    let sent = send(&fx, agent, &submission);
    assert_eq!(send(&fx, agent, &submission), sent);
    assert_eq!(deliveries(&fx, agent), 1);
    assert_eq!(
        standing(&fx, &submission, Some(&sent_under)),
        SubmissionStanding::Recorded
    );
}

/// The guard behind `executed`: a replay-only submission never does the work,
/// whatever became of its receipt between the resolution and the replay.
#[tokio::test]
async fn a_replay_only_submission_without_a_receipt_is_refused_and_does_nothing() {
    let fx = fixture().await;
    let agent = an_agent(&fx).await;
    let submission = message_submission("no-receipt", &agent.to_string(), "ship it");

    let refused = fx.runtime.send_message_keyed(
        agent,
        text("ship it"),
        Actor::user(),
        Some(submission.clone().replay_only()),
    );
    assert!(
        matches!(refused, Err(aether_runtime::RuntimeError::NotFound(_))),
        "{refused:?}"
    );
    assert_eq!(deliveries(&fx, agent), 0);
    assert!(
        fx.store
            .submission(&submission.key)
            .expect("read")
            .is_none(),
        "and it records nothing"
    );
}

#[tokio::test]
async fn a_submission_without_a_horizon_is_never_proven_absent() {
    let fx = fixture().await;
    let agent = an_agent(&fx).await;
    let submission = message_submission("pre-18", &agent.to_string(), "ship it");
    assert_eq!(
        standing(&fx, &submission, None),
        SubmissionStanding::Unproven(UnprovenAbsence::NoHorizon)
    );
}

/// The case the watermark exists for: the command ran, its receipt was evicted
/// by the cap, and "no receipt" must not be read as "never ran".
#[tokio::test]
async fn a_receipt_evicted_by_the_cap_resolves_as_unknown() {
    let fx = capped(2).await;
    let agent = an_agent(&fx).await;
    let sent_under = horizon(&fx);
    let lost = message_submission("evicted", &agent.to_string(), "ship it");
    send(&fx, agent, &lost);
    for i in 0..3 {
        let other = message_submission(&format!("filler-{i}"), &agent.to_string(), "ship it");
        send(&fx, agent, &other);
    }
    assert!(
        fx.store.submission(&lost.key).expect("read").is_none(),
        "the cap evicted the receipt"
    );

    let resolution = fx
        .runtime
        .resolve_submission(&lost, Some(&sent_under))
        .expect("resolve");
    assert_eq!(
        resolution.standing,
        SubmissionStanding::Unproven(UnprovenAbsence::Forgotten)
    );
    assert_eq!(resolution.horizon, horizon(&fx));
    assert_eq!(deliveries(&fx, agent), 4, "and nothing ran again");

    // A key first sent after that eviction is provable again.
    let later = message_submission("after-eviction", &agent.to_string(), "ship it");
    assert_eq!(
        standing(&fx, &later, Some(&resolution.horizon)),
        SubmissionStanding::NeverExecuted
    );
}

/// Evicting receipts older than a key's horizon proves nothing about the key:
/// one busy namespace must not turn every other client's losses into unknowns.
#[tokio::test]
async fn evicting_older_receipts_leaves_a_later_key_provable() {
    let fx = capped(2).await;
    let agent = an_agent(&fx).await;
    for i in 0..2 {
        let old = message_submission(&format!("old-{i}"), &agent.to_string(), "ship it");
        send(&fx, agent, &old);
    }
    let sent_under = horizon(&fx);
    let never_sent = message_submission("never-sent", &agent.to_string(), "ship it");
    let newer = message_submission("newer", &agent.to_string(), "ship it");
    send(&fx, agent, &newer);
    assert!(
        fx.store
            .submission(&IdempotencyKey("old-0".into()))
            .expect("read")
            .is_none(),
        "an older receipt was evicted"
    );
    assert_eq!(
        standing(&fx, &never_sent, Some(&sent_under)),
        SubmissionStanding::NeverExecuted
    );
}

/// Seven days on, the receipt expires in the next keyed commit and the key it
/// answered can no longer be told from one that never ran.
#[tokio::test]
async fn a_receipt_expired_after_seven_days_resolves_as_unknown() {
    let elapsed = Arc::new(std::sync::atomic::AtomicI64::new(0));
    let clock = elapsed.clone();
    let store = SqliteStore::open_in_memory()
        .expect("store")
        .with_retention_clock(move || {
            now() + chrono::Duration::seconds(clock.load(std::sync::atomic::Ordering::SeqCst))
        });
    let fx = over(Arc::new(store)).await;
    let agent = an_agent(&fx).await;
    let sent_under = horizon(&fx);
    let lost = message_submission("expired", &agent.to_string(), "ship it");
    send(&fx, agent, &lost);

    let almost = chrono::Duration::days(7) - chrono::Duration::minutes(1);
    elapsed.store(almost.num_seconds(), std::sync::atomic::Ordering::SeqCst);
    send(
        &fx,
        agent,
        &message_submission("day-6", &agent.to_string(), "ship it"),
    );
    assert_eq!(
        standing(&fx, &lost, Some(&sent_under)),
        SubmissionStanding::Recorded,
        "inside the window the receipt still answers"
    );

    let past = chrono::Duration::days(7) + chrono::Duration::minutes(1);
    elapsed.store(past.num_seconds(), std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        standing(&fx, &lost, Some(&sent_under)),
        SubmissionStanding::Recorded,
        "time alone drops nothing: the sweep runs in a keyed commit"
    );
    send(
        &fx,
        agent,
        &message_submission("day-8", &agent.to_string(), "ship it"),
    );
    assert!(fx.store.submission(&lost.key).expect("read").is_none());
    assert_eq!(
        standing(&fx, &lost, Some(&sent_under)),
        SubmissionStanding::Unproven(UnprovenAbsence::Forgotten)
    );
    assert_eq!(deliveries(&fx, agent), 3);
}

/// A daemon restart loses neither the receipts nor the watermark: every
/// answer is the one the first daemon would have given.
#[tokio::test]
async fn every_outcome_survives_a_restart() {
    let dir = tempfile::tempdir().expect("state dir");
    let path = dir.path().join("state.db");
    let open = || {
        let store = SqliteStore::open(&path)
            .expect("store")
            .with_submission_limits(2, 1);
        Arc::new(store) as Arc<dyn Store>
    };
    let (agent, sent_first, sent_last, evicted, kept, never) = {
        let fx = over(open()).await;
        let agent = an_agent(&fx).await;
        let to = agent.to_string();
        let sent_first = horizon(&fx);
        let evicted = message_submission("evicted", &to, "ship it");
        send(&fx, agent, &evicted);
        send(&fx, agent, &message_submission("filler", &to, "ship it"));
        let sent_last = horizon(&fx);
        let kept = message_submission("kept", &to, "ship it");
        send(&fx, agent, &kept);
        let never = message_submission("never", &to, "ship it");
        fx.runtime.shutdown().await;
        (agent, sent_first, sent_last, evicted, kept, never)
    };

    let fx = over(open()).await;
    assert_eq!(
        standing(&fx, &kept, Some(&sent_last)),
        SubmissionStanding::Recorded
    );
    assert_eq!(
        standing(&fx, &never, Some(&sent_last)),
        SubmissionStanding::NeverExecuted
    );
    assert_eq!(
        standing(&fx, &evicted, Some(&sent_first)),
        SubmissionStanding::Unproven(UnprovenAbsence::Forgotten)
    );
    assert_eq!(deliveries(&fx, agent), 3);
}

/// A different store behind the same address - a fresh database, or one
/// restored from elsewhere - has no account of what the old one executed.
#[tokio::test]
async fn a_replaced_store_resolves_every_absent_key_as_unknown() {
    let old = fixture().await;
    let agent = an_agent(&old).await;
    let sent_under = horizon(&old);
    let lost = message_submission("lost", &agent.to_string(), "ship it");
    send(&old, agent, &lost);

    let replacement = fixture().await;
    assert_eq!(
        standing(&replacement, &lost, Some(&sent_under)),
        SubmissionStanding::Unproven(UnprovenAbsence::ForeignHorizon)
    );
}

/// A store rolled back to an earlier copy of itself has recorded fewer
/// receipts than a client was already told of, and says so.
#[tokio::test]
async fn a_store_rolled_back_behind_the_horizon_resolves_as_unknown() {
    let dir = tempfile::tempdir().expect("state dir");
    let path = dir.path().join("state.db");
    let backup = dir.path().join("backup.db");
    let open = || Arc::new(SqliteStore::open(&path).expect("store"));
    let (lost, sent_under) = {
        let sqlite = open();
        let fx = over(sqlite.clone()).await;
        let agent = an_agent(&fx).await;
        let to = agent.to_string();
        send(&fx, agent, &message_submission("before", &to, "ship it"));
        fx.runtime.shutdown().await;
        assert!(
            sqlite.sync().expect("sync"),
            "the backup is the whole store"
        );
        std::fs::copy(&path, &backup).expect("back up");

        send(&fx, agent, &message_submission("between", &to, "ship it"));
        let sent_under = horizon(&fx);
        let lost = message_submission("lost", &to, "ship it");
        send(&fx, agent, &lost);
        (lost, sent_under)
    };
    // The backup opened in place of the store: the same identity and the
    // same ledger, as they stood before the lost command ran.
    let restored = Arc::new(SqliteStore::open(&backup).expect("restored store"));
    let fx = over(restored).await;
    assert!(fx.store.submission(&lost.key).expect("read").is_none());
    assert_eq!(
        standing(&fx, &lost, Some(&sent_under)),
        SubmissionStanding::Unproven(UnprovenAbsence::HorizonAhead)
    );
}

/// The client's whole recovery - resolve, then resend under the same key -
/// racing the original still in flight: delivered once, whatever the order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resolve_then_resend_racing_the_original_executes_exactly_once() {
    let fx = fixture().await;
    for round in 0..24 {
        let agent = an_agent(&fx).await;
        let sent_under = horizon(&fx);
        let submission = message_submission(&format!("race-{round}"), &agent.to_string(), "go");
        let start = Arc::new(std::sync::Barrier::new(4));
        let mut threads = Vec::new();
        for recovering in [false, true, true, false] {
            let (runtime, submission) = (fx.runtime.clone(), submission.clone());
            let (start, sent_under) = (start.clone(), sent_under.clone());
            threads.push(std::thread::spawn(move || {
                start.wait();
                if recovering {
                    let standing = runtime
                        .resolve_submission(&submission, Some(&sent_under))
                        .expect("resolve")
                        .standing;
                    assert!(
                        !matches!(standing, SubmissionStanding::Unproven(_)),
                        "nothing was dropped: {standing:?}"
                    );
                }
                runtime.send_message_keyed(agent, text("go"), Actor::user(), Some(submission))
            }));
        }
        let ids: Vec<MessageId> = threads
            .into_iter()
            .map(|t| t.join().expect("thread").expect("delivery"))
            .collect();
        assert!(ids.iter().all(|id| *id == ids[0]), "round {round}: {ids:?}");
        assert_eq!(deliveries(&fx, agent), 1, "round {round}");
    }
}
