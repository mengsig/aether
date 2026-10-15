//! Bytes the store writes per streamed event stay bounded however long the
//! history grows (#57). Its own binary, so `/proc/self/io` counts only this test.
#![cfg(target_os = "linux")]
#![allow(clippy::expect_used)]

use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::AgentId;
use aether_store::{Commit, EventFilter, SqliteStore, Store};

static IO_MEASUREMENT: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    let _measurement = IO_MEASUREMENT.lock().expect("measurement lock");
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

/// Isolate the cost of recording 80 streamed usage/context pairs, either one
/// transaction per reading or one absolute pair per 20 readings (a message).
#[test]
fn coalescing_usage_readings_reduces_commits_and_written_bytes() {
    use aether_core::model::Usage;
    let _measurement = IO_MEASUREMENT.lock().expect("measurement lock");
    let agent = AgentId::new();
    let measure = |coalesce: bool| {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.db");
        let store = SqliteStore::open(&path).expect("open");
        assert!(store.sync().expect("checkpoint schema"));
        let before = written();
        let commits = store.commits();
        for n in 1..=80 {
            if coalesce && n % 20 != 0 {
                continue;
            }
            let total = Usage {
                input_tokens: 3 * n,
                output_tokens: n,
                ..Default::default()
            };
            let usage = EventDraft::new(
                Actor::System,
                EventKind::AgentUsageUpdated {
                    total,
                    turn_usage: total,
                },
            )
            .agent(agent);
            let context = EventDraft::new(
                Actor::System,
                EventKind::Custom {
                    name: "context.usage".into(),
                    payload: serde_json::json!({"tokens": 1_000 + n}),
                },
            )
            .agent(agent);
            if coalesce {
                store
                    .commit(Commit::new().event(usage).event(context))
                    .expect("batch");
            } else {
                store.commit(Commit::new().event(usage)).expect("usage");
                store.commit(Commit::new().event(context)).expect("context");
            }
        }
        let commits = store.commits() - commits;
        let bytes = written() - before;
        drop(store);
        let reopened = SqliteStore::open(path).expect("reopen");
        let totals: Vec<_> = reopened
            .events(0, usize::MAX, &EventFilter::for_agent(agent))
            .expect("events")
            .into_iter()
            .filter_map(|event| match event.kind {
                EventKind::AgentUsageUpdated { total, .. } => Some(total),
                _ => None,
            })
            .collect();
        assert_eq!(totals.last().expect("final usage").output_tokens, 80);
        (commits, bytes)
    };
    let before = measure(false);
    let after = measure(true);
    eprintln!(
        "80 usage/context pairs: before {} commits / {} written bytes; after {} commits / {} written bytes",
        before.0, before.1, after.0, after.1
    );
    assert_eq!(before.0, 160);
    assert_eq!(after.0, 4);
    assert!(
        after.1 < before.1 / 10,
        "before {before:?}, after {after:?}"
    );
}
