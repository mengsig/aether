use super::*;
use std::sync::mpsc;
use std::time::Duration;

fn file_store() -> (tempfile::TempDir, SqliteStore) {
    let dir = tempfile::tempdir().expect("tmp");
    let store = SqliteStore::open(dir.path().join("aether.db")).expect("open");
    store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(agent())))
        .expect("seed");
    (dir, store)
}

/// One shared reader made `status` and `get_tree` wait for every read before
/// theirs, a long-lived agent's whole transcript included (#333).
#[test]
fn a_read_does_not_wait_for_another_read_in_progress() {
    let (_dir, store) = file_store();
    let (done, finished) = mpsc::channel();
    std::thread::scope(|scope| {
        let mut held = store.read_conn().expect("a read connection");
        let transaction = held.transaction().expect("a read transaction");
        let agents: i64 = transaction
            .query_row("SELECT COUNT(*) FROM agents", [], |row| row.get(0))
            .expect("count");
        assert_eq!(agents, 1);
        scope.spawn(|| {
            let read = store.agents(&AgentFilter::default()).map(|a| a.len());
            let _ = done.send((read, store.last_seq()));
        });
        let answer = finished.recv_timeout(Duration::from_secs(2));
        // Released before the scope joins, so a blocked reader cannot hang the test.
        drop(transaction);
        drop(held);
        let (agents, last_seq) = answer.expect("the second read waited for the first");
        assert_eq!(agents.expect("agents"), 1);
        assert_eq!(last_seq.expect("last seq"), 0, "the seed wrote no event");
    });
}

/// Past `MAX_OPEN` concurrent reads, the next waits for a connection to come
/// back rather than opening file descriptors without bound.
#[test]
fn reads_past_the_connection_cap_wait_for_one_to_return() {
    let (_dir, store) = file_store();
    let mut leases: Vec<_> = (0..read_pool::MAX_OPEN)
        .map(|_| store.read_conn().expect("a read connection"))
        .collect();
    let (done, finished) = mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let _ = done.send(store.last_seq());
        });
        let early = finished.recv_timeout(Duration::from_millis(200));
        leases.pop();
        let late = finished.recv_timeout(Duration::from_secs(5));
        leases.clear();
        assert!(early.is_err(), "a read opened a connection past the cap");
        assert!(late.expect("the read took the returned connection").is_ok());
    });
    let leased_again = store.read_conn().expect("connections are reused");
    assert!(leased_again.is_autocommit());
}
