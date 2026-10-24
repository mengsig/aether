use super::*;
use std::time::{Duration, Instant};

/// Commit `megabytes` through the store's connection, one row per commit: well
/// past SQLite's automatic checkpoint threshold of 1000 pages.
fn commit_megabytes(store: &SqliteStore, megabytes: usize) {
    let conn = store.conn.lock();
    conn.execute_batch("CREATE TABLE IF NOT EXISTS ballast(body BLOB NOT NULL)")
        .expect("table");
    for _ in 0..megabytes * 4 {
        conn.execute("INSERT INTO ballast(body) VALUES (zeroblob(262144))", [])
            .expect("insert");
    }
}

fn file_len(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).expect("metadata").len()
}

/// A checkpoint fsyncs the database, which took 15-30 s on a contended disk.
/// Run inside COMMIT it held the store's mutex and the write gate that long,
/// and every request waited. In WAL mode only a checkpoint writes the
/// database file, so its size shows whether one ran.
#[test]
fn a_commit_leaves_the_checkpoint_to_the_checkpointer() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("aether.db");
    let store = SqliteStore::open_checkpointing(&path, Duration::from_secs(3600)).expect("open");
    let before = file_len(&path);
    commit_megabytes(&store, 8);
    assert_eq!(
        file_len(&path),
        before,
        "no commit copied the WAL into the database"
    );
    let wal = path.with_extension("db-wal");
    assert!(file_len(&wal) >= 8 << 20, "the commits are in the WAL");
}

#[test]
fn the_checkpointer_copies_the_wal_into_the_database() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("aether.db");
    let store = SqliteStore::open_checkpointing(&path, Duration::from_millis(20)).expect("open");
    commit_megabytes(&store, 2);
    let size: u64 = store
        .conn
        .lock()
        .query_row(
            "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
            [],
            |r| r.get::<_, i64>(0),
        )
        .expect("size") as u64;
    let deadline = Instant::now() + Duration::from_secs(30);
    while file_len(&path) < size {
        assert!(
            Instant::now() < deadline,
            "the database file reached {} of {size} bytes",
            file_len(&path)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One large write left a 165 MB WAL on disk for good, because a passive
/// checkpoint reuses the file without shrinking it.
#[test]
fn the_checkpointer_truncates_a_large_wal_once_it_is_copied() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("aether.db");
    let store = SqliteStore::open_checkpointing(&path, Duration::from_millis(20)).expect("open");
    commit_megabytes(&store, 24);
    let wal = path.with_extension("db-wal");
    let deadline = Instant::now() + Duration::from_secs(30);
    while file_len(&wal) > 1 << 20 {
        assert!(
            Instant::now() < deadline,
            "the WAL is still {} bytes",
            file_len(&wal)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The first commit after the checkpointer empties the WAL still syncs the new
/// WAL header. fsync(2) waits on the ext4 journal (30 s under load); fdatasync
/// does not. `.cargo/config.toml` sets this for the bundled SQLite build.
#[test]
fn bundled_sqlite_syncs_with_fdatasync() {
    assert!(
        option_env!("LIBSQLITE3_FLAGS").is_some_and(|flags| flags.contains("-DHAVE_FDATASYNC=1")),
        "LIBSQLITE3_FLAGS must carry -DHAVE_FDATASYNC=1 into the SQLite build"
    );
}

/// A commit holds the write connection for as long as its WAL sync takes,
/// seconds on a busy disk. Reads, which most requests are, must not queue.
#[test]
fn a_read_does_not_wait_for_a_commit_in_progress() {
    let dir = tempfile::tempdir().expect("tmp");
    let store = SqliteStore::open(dir.path().join("aether.db")).expect("open");
    let a = agent();
    store
        .commit(Commit::new().mutate(Mutation::UpsertAgent(a.clone())))
        .expect("commit");
    let writing = store.conn.lock();
    let (sender, read) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| sender.send(store.agents(&AgentFilter::default())));
        let agents = read.recv_timeout(Duration::from_secs(10));
        drop(writing);
        let agents = agents
            .expect("the read finished while the write connection was held")
            .expect("agents");
        assert_eq!(agents.iter().map(|x| x.id).collect::<Vec<_>>(), [a.id]);
    });
}

/// The reads behind `list_agents`, `list_tasks`, `read_task` and
/// `inspect_state` all answer while a commit holds the write connection.
#[test]
fn the_read_tools_queries_do_not_wait_for_a_commit_in_progress() {
    let dir = tempfile::tempdir().expect("tmp");
    let store = SqliteStore::open(dir.path().join("aether.db")).expect("open");
    let a = agent();
    let mut task = Task::new("t", Actor::user(), a.run, now());
    task.owner = Some(a.id);
    store
        .commit(
            Commit::new()
                .mutate(Mutation::UpsertAgent(a.clone()))
                .mutate(Mutation::UpsertTask(task.clone())),
        )
        .expect("commit");
    let writing = store.conn.lock();
    let (sender, read) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let reads = (|| -> Result<_> {
                Ok((
                    store.agent(a.id)?.map(|x| x.id),
                    store.tasks(&TaskFilter::default())?.len(),
                    store.task(task.id)?.map(|t| t.id),
                    store.subtree_ids(a.id)?.len(),
                ))
            })();
            sender.send(reads)
        });
        let reads = read.recv_timeout(Duration::from_secs(10));
        drop(writing);
        let reads = reads
            .expect("the reads finished while the write connection was held")
            .expect("reads");
        assert_eq!(reads, (Some(a.id), 1, Some(task.id), 1));
    });
}
