//! Bytes the store writes per streamed event stay bounded however long the
//! history grows (#57). Its own binary, so `/proc/self/io` counts only this test.
#![cfg(target_os = "linux")]
#![allow(clippy::expect_used)]

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::AgentId;
use aether_store::{Commit, EventFilter, SqliteStore, Store};

/// Enough history that a sort over it spills far past SQLite's 2 MB cache.
const HISTORY: usize = 20_000;
const STREAMED: usize = 10;
/// A commit touches the events table, its indexes and `meta`: a few dozen pages.
const MAX_BYTES_PER_EVENT: u64 = 256 * 1024;

/// Bytes this process handed to `write(2)` and friends, on any filesystem.
fn written() -> u64 {
    let io = std::fs::read_to_string("/proc/self/io").expect("read /proc/self/io");
    io.lines()
        .find_map(|line| line.strip_prefix("wchar: "))
        .and_then(|n| n.parse().ok())
        .expect("wchar in /proc/self/io")
}

fn usage(agent: AgentId, parent: Option<AgentId>, padding: &str) -> EventDraft {
    EventDraft::new(
        Actor::System,
        EventKind::Custom {
            name: "context.usage".into(),
            payload: serde_json::json!({ "tokens": 1, "padding": padding }),
        },
    )
    .agent(agent)
    .parent(parent)
}

#[test]
fn reading_a_long_history_while_streaming_writes_only_the_new_events() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::open(dir.path().join("state.db")).expect("open store");
    let (parent, child) = (AgentId::new(), AgentId::new());
    let padding = "x".repeat(1024);
    let mut seed = Commit::new();
    for n in 0..HISTORY {
        seed = seed.event(match n % 2 {
            0 => usage(parent, None, &padding),
            _ => usage(child, Some(parent), &padding),
        });
    }
    store.commit(seed).expect("seed history");
    assert!(store.sync().expect("checkpoint the seed"));

    let before = written();
    for _ in 0..STREAMED {
        store
            .commit(Commit::new().event(usage(parent, None, "")))
            .expect("commit a streamed event");
        // What a client following the parent and its children reads: the
        // newest page, merged from two index branches, and the whole log.
        let page = store
            .latest_events(2_000, &EventFilter::for_agent(parent))
            .expect("newest page");
        assert_eq!(page.len(), 2_000);
        let all = store
            .events(0, usize::MAX, &EventFilter::for_agent(parent))
            .expect("whole history");
        assert!(all.len() > HISTORY);
    }
    let per_event = (written() - before) / STREAMED as u64;
    assert!(
        per_event <= MAX_BYTES_PER_EVENT,
        "{per_event} bytes written per streamed event, over {MAX_BYTES_PER_EVENT}"
    );
}
