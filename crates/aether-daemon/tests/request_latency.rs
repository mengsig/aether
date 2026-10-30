#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! `status` and `get_tree` must answer while the daemon does slow work (#333):
//! behind a slow request on the same connection, and behind long history reads.

mod support;

use std::time::{Duration, Instant};

use aether_core::agent::{Agent, AgentStatus};
use aether_core::event::{Actor, EventDraft, EventKind};
use aether_core::id::{AgentId, RunId};
use aether_core::model::{ContentBlock, MessageRole};
use aether_protocol::Command;
use aether_store::{Commit, Mutation, SqliteStore, Store};
use serde_json::{Value, json};
use support::{RawConn, TestDaemon};

/// What the issue asks of a read the TUI and Archon poll.
const READ_BOUND: Duration = Duration::from_secs(2);

async fn connect(daemon: &TestDaemon) -> RawConn {
    let mut connection = RawConn::connect(&daemon.socket).await.unwrap();
    connection
        .hello(aether_protocol::PROTOCOL_VERSION)
        .await
        .unwrap();
    connection
}

fn frame(command: &Command) -> Value {
    serde_json::to_value(command).unwrap()
}

fn create_agent() -> Value {
    json!({"cmd": "create_agent", "role": "latency-test", "start": false,
        "model": {"provider": "claude-code", "model": "haiku"}})
}

/// Replies to `wanted` that arrive within `bound`, by request id; any other
/// reply read on the way is returned too.
async fn replies_within(connection: &mut RawConn, wanted: &[u64], bound: Duration) -> Vec<u64> {
    let mut seen = Vec::new();
    let deadline = Instant::now() + bound;
    while !wanted.iter().all(|id| seen.contains(id)) {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(Some(answer)) = tokio::time::timeout(left, connection.recv()).await else {
            break;
        };
        if answer["frame"] == "reply" {
            seen.push(answer["id"].as_u64().expect("a reply id"));
        }
    }
    seen
}

/// A write that waits out a database another process holds locked used to
/// hold every later request on its connection, the TUI's polls included.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reads_behind_a_stalled_write_on_the_same_connection_answer_at_once() {
    let daemon = TestDaemon::start().await;
    let mut connection = connect(&daemon).await;
    let locker = rusqlite::Connection::open(daemon.store_path()).unwrap();
    locker.execute_batch("BEGIN IMMEDIATE").unwrap();

    let write = connection.request(create_agent()).await;
    let status = connection.request(frame(&Command::Status)).await;
    let tree = connection
        .request(frame(&Command::GetTree {
            run: None,
            include_archived: false,
        }))
        .await;
    let answered = replies_within(&mut connection, &[status, tree], READ_BOUND).await;
    assert!(
        answered.contains(&status) && answered.contains(&tree),
        "status and get_tree waited behind the stalled write: answered {answered:?}"
    );
    assert!(
        !answered.contains(&write),
        "the write answered while the database was still locked"
    );

    locker.execute_batch("ROLLBACK").unwrap();
    let answered = replies_within(&mut connection, &[write], Duration::from_secs(30)).await;
    assert_eq!(answered, [write], "the write answers once the lock is gone");
    daemon.stop().await;
}

/// `shutdown` still waits for every command sent before it, though reads no
/// longer do: the write it follows is applied and answered first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_runs_after_the_writes_sent_before_it() {
    let daemon = TestDaemon::start().await;
    let mut connection = connect(&daemon).await;
    let locker = rusqlite::Connection::open(daemon.store_path()).unwrap();
    locker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let write = connection.request(create_agent()).await;
    let shutdown = connection.request(json!({"cmd": "shutdown"})).await;
    let early = replies_within(&mut connection, &[shutdown], Duration::from_millis(500)).await;
    assert!(
        early.is_empty(),
        "answered behind a locked write: {early:?}"
    );

    locker.execute_batch("ROLLBACK").unwrap();
    let mut order = Vec::new();
    while let Ok(Some(answer)) =
        tokio::time::timeout(Duration::from_secs(30), connection.recv()).await
    {
        if answer["frame"] == "reply" {
            order.push(answer["id"].as_u64().expect("a reply id"));
            assert!(answer["result"]["Ok"].is_object(), "{answer}");
        }
    }
    assert_eq!(order, [write, shutdown], "the write answers, then shutdown");
    daemon.stop().await;
}

/// A store shaped like a long-used one: many finished agents and a few
/// long-lived ones whose transcripts run to tens of megabytes.
struct Seeded {
    long_lived: Vec<AgentId>,
    bytes: u64,
}

const MESSAGE_BYTES: usize = 4096;
const MESSAGES_PER_COMMIT: usize = 500;
/// About 25 MB of transcript: the longest on a long-used store, well under
/// the 64 MiB a reply may carry.
const LONG_LIVED_MESSAGES: usize = 6000;

fn message(agent: &Agent, n: usize) -> EventDraft {
    let text = format!("{n:08} ") + &"history ".repeat(MESSAGE_BYTES / 8);
    EventDraft::new(
        Actor::System,
        EventKind::AgentMessage {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::text(text)],
            invocation: None,
        },
    )
    .agent(agent.id)
    .run(agent.run)
}

fn clone_agent(template: &Agent, run: RunId, status: AgentStatus) -> Agent {
    let mut agent = template.clone();
    agent.id = AgentId::new();
    agent.run = run;
    agent.status = status;
    agent.paused = true;
    agent
}

fn write_history(store: &SqliteStore, agent: &Agent, messages: usize) {
    let mut written = 0;
    while written < messages {
        let batch = MESSAGES_PER_COMMIT.min(messages - written);
        let mut commit = Commit::new();
        for n in written..written + batch {
            commit.push_event(message(agent, n));
        }
        store.commit(commit).unwrap();
        written += batch;
    }
}

/// Grow the store at `path` to about `megabytes`: `long_lived` agents with
/// long transcripts, and the rest spread over many finished agents.
fn seed(path: &std::path::Path, template: &Agent, megabytes: usize, long_lived: usize) -> Seeded {
    let store = SqliteStore::open(path).unwrap();
    let finished: Vec<Agent> = (0..1500)
        .map(|n| {
            let run = RunId::new();
            let status = if n % 4 == 0 {
                AgentStatus::Cancelled { reason: None }
            } else {
                AgentStatus::Completed { summary: None }
            };
            clone_agent(template, run, status)
        })
        .collect();
    let long: Vec<Agent> = (0..long_lived)
        .map(|_| clone_agent(template, RunId::new(), AgentStatus::Created))
        .collect();
    let mut commit = Commit::new();
    for agent in finished.iter().chain(&long) {
        commit = commit.mutate(Mutation::UpsertAgent(agent.clone()));
    }
    store.commit(commit).unwrap();
    let total_messages = megabytes * 1024 * 1024 / MESSAGE_BYTES;
    for agent in &long {
        write_history(&store, agent, LONG_LIVED_MESSAGES);
    }
    let rest = total_messages.saturating_sub(LONG_LIVED_MESSAGES * long.len());
    let per_finished = (rest / finished.len()).max(1);
    for agent in &finished {
        write_history(&store, agent, per_finished);
    }
    drop(store);
    let wal = path.with_extension("db-wal");
    let bytes = [path, wal.as_path()]
        .iter()
        .filter_map(|file| std::fs::metadata(file).ok())
        .map(|metadata| metadata.len())
        .sum();
    Seeded {
        long_lived: long.iter().map(|agent| agent.id).collect(),
        bytes,
    }
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    assert!(!sorted.is_empty(), "a percentile of no samples");
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn report(name: &str, mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    let max = *samples.last().expect("samples");
    eprintln!(
        "{name:>14}: n={:4} p50={:>8.1?} p95={:>8.1?} max={max:>8.1?}",
        samples.len(),
        percentile(&samples, 0.5),
        percentile(&samples, 0.95),
    );
    max
}

async fn timed(client: &aether_client::Client, command: Command) -> Duration {
    let started = Instant::now();
    client.request(command).await.expect("request");
    started.elapsed()
}

/// Long transcript reads, as long-lived agents' turns and a TUI showing one
/// make them, until `until`.
async fn read_transcripts(
    client: std::sync::Arc<aether_client::Client>,
    agents: Vec<AgentId>,
    until: Instant,
) -> Vec<Duration> {
    let mut samples = Vec::new();
    let mut n = 0;
    while Instant::now() < until {
        let agent = agents[n % agents.len()];
        n += 1;
        samples.push(timed(&client, Command::GetTranscript { agent, after: 0 }).await);
    }
    samples
}

/// The TUI's poll on its one connection, with its own transcript reads beside it.
async fn poll_like_the_tui(
    client: std::sync::Arc<aether_client::Client>,
    until: Instant,
) -> (Vec<Duration>, Vec<Duration>) {
    let (mut status, mut tree) = (Vec::new(), Vec::new());
    while Instant::now() < until {
        status.push(timed(&client, Command::Status).await);
        let get_tree = Command::GetTree {
            run: None,
            include_archived: false,
        };
        tree.push(timed(&client, get_tree).await);
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    (status, tree)
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .map(|value| value.parse().expect("a number"))
        .unwrap_or(default)
}

/// The reproduction for #333 on a store of `AETHER_BENCH_STORE_MB` (default
/// 1500): run with `--ignored --nocapture`; it prints each read's latency.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "a benchmark: seeds a store of over a gigabyte"]
async fn bench_status_and_get_tree_on_a_large_busy_store() {
    let megabytes = env_usize("AETHER_BENCH_STORE_MB", 1500);
    let seconds = env_usize("AETHER_BENCH_SECONDS", 30) as u64;
    let readers = env_usize("AETHER_BENCH_READERS", 6);
    let daemon = TestDaemon::start().await;
    let mut raw = connect(&daemon).await;
    let created = raw.call(create_agent()).await;
    let template: Agent = serde_json::from_value(created["result"]["Ok"]["data"].clone()).unwrap();
    drop(raw);
    let seeding = Instant::now();
    let mut seeded = None;
    // Seeded while the daemon is stopped, so its readers cannot hold the WAL open.
    let (daemon, startup) = daemon
        .restart_large_after(|path| seeded = Some(seed(path, &template, megabytes, 12)))
        .await;
    let seeded = seeded.expect("seeded");
    eprintln!(
        "seeded {} MB, then the daemon answered after {startup:.1?} ({:.1?} in all)",
        seeded.bytes >> 20,
        seeding.elapsed()
    );

    let until = Instant::now() + Duration::from_secs(seconds);
    let mut heavy = Vec::new();
    for n in 0..readers {
        let client = std::sync::Arc::new(
            aether_client::Client::connect(&daemon.socket)
                .await
                .unwrap(),
        );
        let mut agents = seeded.long_lived.clone();
        let turn = n % agents.len();
        agents.rotate_left(turn);
        heavy.push(tokio::spawn(read_transcripts(client, agents, until)));
    }
    let tui = std::sync::Arc::new(
        aether_client::Client::connect(&daemon.socket)
            .await
            .unwrap(),
    );
    let own_reads = tokio::spawn(read_transcripts(
        tui.clone(),
        seeded.long_lived[..1].to_vec(),
        until,
    ));
    let (status, tree) = poll_like_the_tui(tui, until).await;
    let mut transcripts = own_reads.await.unwrap();
    for reader in heavy {
        transcripts.extend(reader.await.unwrap());
    }
    report("get_transcript", transcripts);
    let status_max = report("status", status);
    let tree_max = report("get_tree", tree);
    daemon.stop().await;
    assert!(
        status_max < READ_BOUND && tree_max < READ_BOUND,
        "status took up to {status_max:?} and get_tree {tree_max:?}"
    );
}
