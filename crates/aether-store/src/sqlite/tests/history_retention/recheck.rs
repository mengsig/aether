//! Expiry under the commit hold judges only what was written since its run
//! was screened; judging whole tables there stalled every commit (#314).
use super::*;
use aether_core::id::{AgentId, InboxItemId};
use aether_core::inbox::{InboxItem, InboxKind};

fn deliver(f: &Fixture, recipient: AgentId, kind: InboxKind) {
    f.mutate(Mutation::DeliverInbox(InboxItem {
        id: InboxItemId::new(),
        agent_id: recipient,
        seq: 1,
        kind,
        delivered_at: f.agent.created_at,
    }));
}

fn file_fixture(dir: &tempfile::TempDir) -> Fixture {
    Fixture::on(SqliteStore::open(dir.path().join("aether.db")).expect("open"))
}

fn eligible_screen(f: &Fixture) -> crate::HistoryScreen {
    let screen = f
        .store
        .screen_run_expiry(f.agent.run, f.cutoff)
        .expect("screen");
    assert_eq!(screen.blocker(), None, "the seeded run may expire");
    assert_eq!(screen.run(), f.agent.run);
    screen
}

#[test]
fn expiry_after_a_screen_reads_no_whole_table_under_the_hold() {
    let dir = tempfile::tempdir().expect("tmp");
    let f = file_fixture(&dir);
    let screen = eligible_screen(&f);
    let recipient = agent();
    f.mutate(Mutation::UpsertAgent(recipient.clone()));
    deliver(
        &f,
        recipient.id,
        InboxKind::Wake {
            reason: "unrelated".into(),
        },
    );
    // The checkpointer's own connection copies the WAL meanwhile.
    let path = f.store.path.clone().expect("a file store");
    let checkpointer = Connection::open(path).expect("connection");
    let (busy, frames, copied): (i64, i64, i64) = checkpointer
        .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .expect("checkpoint");
    assert_eq!((busy, copied), (0, frames), "the whole WAL was copied");
    let passes = f.store.history_guard_passes();
    let verdict = f
        .store
        .expire_screened_run(&screen, &f.gate.hold())
        .expect("expiry");
    assert!(matches!(verdict, RunExpiry::Expired { .. }), "{verdict:?}");
    assert_eq!(
        f.store.history_guard_passes(),
        passes,
        "a whole-table guard pass ran under the hold"
    );
    assert_eq!(f.store.inbox(recipient.id, 0).expect("inbox").len(), 1);
}

#[test]
fn a_reference_written_after_the_screen_keeps_the_run() {
    let f = Fixture::new();
    let screen = eligible_screen(&f);
    let recipient = agent();
    f.mutate(Mutation::UpsertAgent(recipient.clone()));
    let task = f.task.id;
    deliver(
        &f,
        recipient.id,
        InboxKind::TaskCancelled { task, reason: None },
    );
    let passes = f.store.history_guard_passes();
    assert_eq!(
        f.store
            .expire_screened_run(&screen, &f.gate.hold())
            .expect("expiry"),
        RunExpiry::Protected("external inbox reference")
    );
    assert_eq!(
        f.store.history_guard_passes(),
        passes,
        "judged by the written rows alone"
    );
    f.assert_preserved();
}

#[test]
fn a_state_change_after_the_screen_keeps_the_run() {
    let f = Fixture::new();
    let screen = eligible_screen(&f);
    let mut agent = f.agent.clone();
    agent.status = AgentStatus::Ready;
    f.mutate(Mutation::UpsertAgent(agent));
    assert_eq!(
        f.store
            .expire_screened_run(&screen, &f.gate.hold())
            .expect("expiry"),
        RunExpiry::Protected("unfinished work")
    );
    f.assert_preserved();
}

/// An inbox row the screen read unchanged names a task the run gains only
/// later, so only a full pass can see the reference.
#[test]
fn a_member_added_since_the_screen_rechecks_every_guard() {
    let f = Fixture::new();
    let recipient = agent();
    f.mutate(Mutation::UpsertAgent(recipient.clone()));
    let old = f.agent.updated_at;
    let mut late = Task::new("late task", Actor::agent(f.agent.id), f.agent.run, old);
    late.status = TaskStatus::Completed {
        result: TaskResult::summary("finished"),
    };
    let task = late.id;
    deliver(
        &f,
        recipient.id,
        InboxKind::TaskCancelled { task, reason: None },
    );
    let screen = eligible_screen(&f);
    f.mutate(Mutation::UpsertTask(late));
    let passes = f.store.history_guard_passes();
    assert_eq!(
        f.store
            .expire_screened_run(&screen, &f.gate.hold())
            .expect("expiry"),
        RunExpiry::Protected("external inbox reference")
    );
    assert_eq!(f.store.history_guard_passes(), passes + 1);
}

/// Another connection's rows never reach this store's change log.
#[test]
fn a_commit_by_another_connection_rechecks_every_guard() {
    let dir = tempfile::tempdir().expect("tmp");
    let f = file_fixture(&dir);
    let screen = eligible_screen(&f);
    let other = SqliteStore::open(dir.path().join("aether.db")).expect("second store");
    let recipient = agent();
    let task = f.task.id;
    other
        .commit(
            Commit::new()
                .mutate(Mutation::UpsertAgent(recipient.clone()))
                .mutate(Mutation::DeliverInbox(InboxItem {
                    id: InboxItemId::new(),
                    agent_id: recipient.id,
                    seq: 1,
                    kind: InboxKind::TaskCancelled { task, reason: None },
                    delivered_at: f.agent.created_at,
                })),
        )
        .expect("commit");
    let passes = f.store.history_guard_passes();
    assert_eq!(
        f.store
            .expire_screened_run(&screen, &f.gate.hold())
            .expect("expiry"),
        RunExpiry::Protected("external inbox reference")
    );
    assert_eq!(f.store.history_guard_passes(), passes + 1);
}

/// A file store whose one old run sits beside `inbox` unrelated inbox rows.
fn large_fixture(dir: &tempfile::TempDir, inbox: usize) -> Fixture {
    let f = file_fixture(dir);
    let recipient = agent();
    f.mutate(Mutation::UpsertAgent(recipient.clone()));
    for batch in 0..inbox.div_ceil(5_000) {
        let mut commit = Commit::new();
        for seq in batch * 5_000..inbox.min((batch + 1) * 5_000) {
            commit = commit.mutate(Mutation::DeliverInbox(InboxItem {
                id: InboxItemId::new(),
                agent_id: recipient.id,
                seq: seq as u64 + 1,
                kind: InboxKind::Wake {
                    reason: format!("unrelated wake {seq}"),
                },
                delivered_at: f.agent.created_at,
            }));
        }
        f.store.commit(commit).expect("inbox batch");
    }
    f
}

/// `cargo test --release -p aether-store --lib -- --ignored --nocapture
/// expiry_hold_time`: how long one run's expiry holds the commit gate.
#[test]
#[ignore = "timing bench"]
fn expiry_hold_time_with_a_hundred_thousand_inbox_rows() {
    const INBOX: usize = 100_000;
    const WRITTEN: usize = 200;
    let full_dir = tempfile::tempdir().expect("tmp");
    let full = large_fixture(&full_dir, INBOX);
    let started = std::time::Instant::now();
    let verdict = full
        .store
        .expire_run_history(full.agent.run, full.cutoff, &full.gate.hold())
        .expect("expiry");
    let full_hold = started.elapsed();
    assert!(matches!(verdict, RunExpiry::Expired { .. }), "{verdict:?}");

    let dir = tempfile::tempdir().expect("tmp");
    let f = large_fixture(&dir, INBOX);
    let started = std::time::Instant::now();
    let screen = eligible_screen(&f);
    let screened = started.elapsed();
    let recipient = agent();
    f.mutate(Mutation::UpsertAgent(recipient.clone()));
    for seq in 1..=WRITTEN as u64 {
        f.mutate(Mutation::DeliverInbox(InboxItem {
            id: InboxItemId::new(),
            agent_id: recipient.id,
            seq,
            kind: InboxKind::Wake {
                reason: "since the screen".into(),
            },
            delivered_at: f.agent.created_at,
        }));
    }
    let started = std::time::Instant::now();
    let verdict = f
        .store
        .expire_screened_run(&screen, &f.gate.hold())
        .expect("expiry");
    let hold = started.elapsed();
    assert!(matches!(verdict, RunExpiry::Expired { .. }), "{verdict:?}");
    eprintln!(
        "{INBOX} inbox rows: hold {full_hold:?} judging under the gate, \
         {hold:?} after a screen of {screened:?} outside it and {WRITTEN} writes since"
    );
    if !cfg!(debug_assertions) {
        assert!(
            hold < std::time::Duration::from_millis(20),
            "held the gate {hold:?}"
        );
    }
}
