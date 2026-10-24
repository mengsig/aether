//! Synthetic load for `aether status` write accounting: several streaming
//! agents, each reporting usage many times a second and running tools with
//! large output. Run with `--nocapture` for the per-kind table.
use super::*;
use aether_core::model::{StreamEvent, Usage};
use aether_core::write_stats::WriteStats;
use aether_store::{Commit, Mutation};

const AGENTS: usize = 6;
const MESSAGES: u64 = 3;
const READINGS_PER_MESSAGE: u64 = 24;
const READING_GAP: Duration = Duration::from_millis(125);
const TOOL_OUTPUT_BYTES: usize = 48 * 1024;

/// A turn the way a busy Claude Code session streams one: bursts of usage and
/// context readings, text deltas, a tool call per message with a big result.
struct StreamingEngine;

impl StreamingEngine {
    fn tool_output(message: u64) -> String {
        (0..TOOL_OUTPUT_BYTES / 64)
            .map(|line| format!("{message:04}:{line:06} build output line with some payload\n"))
            .collect()
    }
}

#[async_trait::async_trait]
impl Engine for StreamingEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }
    fn adapter(&self) -> String {
        "streaming-load/1".into()
    }
    async fn run(
        &self,
        _turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            let mut context = 1_000;
            let mut total = Usage::default();
            for message in 0..MESSAGES {
                for _ in 0..READINGS_PER_MESSAGE {
                    let usage = Usage {
                        input_tokens: 30,
                        output_tokens: 10,
                        ..Default::default()
                    };
                    total += usage;
                    context += 1;
                    let _ = tx.send(EngineEvent::UsageDelta { usage }).await;
                    let _ = tx.send(EngineEvent::ContextUsage { tokens: context }).await;
                    let _ = tx
                        .send(EngineEvent::Delta(StreamEvent::TextDelta {
                            index: 0,
                            text: "streamed token ".into(),
                        }))
                        .await;
                    tokio::time::sleep(READING_GAP).await;
                }
                let id = format!("toolu_{message}");
                let _ = tx
                    .send(EngineEvent::Assistant {
                        content: vec![
                            ContentBlock::text(format!("working on step {message}")),
                            ContentBlock::ToolUse {
                                id: id.clone(),
                                name: "bash".into(),
                                input: serde_json::json!({"command": format!("make step{message}")}),
                            },
                        ],
                        usage: None,
                        model: None,
                    })
                    .await;
                let _ = tx
                    .send(EngineEvent::ToolResult {
                        tool_use_id: id,
                        content: Self::tool_output(message),
                        is_error: false,
                    })
                    .await;
            }
            let _ = tx
                .send(EngineEvent::Done(EngineOutcome {
                    closing_accounting: true,
                    text: Some("done".into()),
                    usage: total,
                    ..Default::default()
                }))
                .await;
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

/// Bytes this process handed to `write(2)`, on any filesystem. Linux only:
/// elsewhere the load still runs and only the store's own tally is checked.
fn written() -> Option<u64> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let io = std::fs::read_to_string("/proc/self/io").expect("read /proc/self/io");
    let wchar = io
        .lines()
        .find_map(|line| line.strip_prefix("wchar: "))
        .and_then(|n| n.parse().ok())
        .expect("wchar in /proc/self/io");
    Some(wchar)
}

fn report(stats: &WriteStats, elapsed: Duration, os_bytes: Option<u64>) {
    let minutes = elapsed.as_secs_f64() / 60.0;
    eprintln!(
        "load: {AGENTS} agents, {:.1}s; payload {} B in {} rows ({:.0} B/min); process wchar {os_bytes:?} B ({:?} B/min); db {:?} wal {:?}",
        elapsed.as_secs_f64(),
        stats.total_bytes,
        stats.total_rows,
        stats.total_bytes as f64 / minutes,
        os_bytes.map(|bytes| (bytes as f64 / minutes).round()),
        stats.db_bytes,
        stats.wal_bytes,
    );
    for kind in stats.clone().top(12).kinds {
        eprintln!(
            "  {:<34} {:>7} rows {:>10} B",
            kind.kind, kind.rows, kind.bytes
        );
    }
}

/// What one load wrote: the store's own tally, its commits, and the bytes
/// the whole process wrote (meaningful only when the test runs alone).
struct Load {
    stats: WriteStats,
    commits: u64,
    os_bytes: Option<u64>,
    elapsed: Duration,
}

async fn run_load(token_budget: Option<u64>) -> Load {
    let dir = tempfile::tempdir().expect("tempdir");
    let store: Arc<dyn Store> =
        Arc::new(SqliteStore::open(dir.path().join("state.db")).expect("open store"));
    let h = build_harness(
        Arc::new(StreamingEngine),
        Arc::default(),
        Arc::default(),
        None,
        None,
        Some(store.clone()),
        |config| config.max_concurrent_turns = AGENTS,
    )
    .await;
    let agents: Vec<_> = (0..AGENTS).map(|_| chat_agent(&h)).collect();
    if token_budget.is_some() {
        for id in &agents {
            let mut agent = store.agent(*id).expect("read").expect("agent");
            agent.config.token_budget = token_budget;
            h.runtime
                .apply(Commit::new().mutate(Mutation::UpsertAgent(agent)))
                .expect("budget");
        }
    }
    let (before, started, commits) = (written(), std::time::Instant::now(), store.commits());
    for id in &agents {
        h.runtime
            .send_message(*id, vec![ContentBlock::text("Go")], Actor::user())
            .expect("send");
    }
    for id in &agents {
        let agent = wait_for(store.as_ref(), *id, |a| {
            a.stats.turns == 1 && a.status.kind() == AgentStatusKind::Waiting
        })
        .await;
        assert_eq!(
            agent.stats.usage.output_tokens,
            MESSAGES * READINGS_PER_MESSAGE * 10,
            "coalescing loses no usage"
        );
    }
    let load = Load {
        stats: store.write_stats(),
        commits: store.commits() - commits,
        os_bytes: written().zip(before).map(|(after, before)| after - before),
        elapsed: started.elapsed(),
    };
    h.runtime.shutdown().await;
    load
}

/// Bounds the store's own counts, not process I/O, which concurrent tests
/// share. Run alone with `--exact --nocapture` for the per-minute figures.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn streaming_agents_write_a_bounded_volume() {
    for (label, budget) in [("unbudgeted", None), ("budgeted", Some(u64::MAX / 2))] {
        let load = run_load(budget).await;
        eprintln!("{label}: {} commits", load.commits);
        report(&load.stats, load.elapsed, load.os_bytes);
        let usage = load
            .stats
            .kinds
            .iter()
            .find(|kind| kind.kind == "agent.usage_updated")
            .map_or(0, |kind| kind.rows);
        assert!(usage > 0, "{label}: usage reaches the record");
        // Readings coalesce to about one commit per agent per second at most,
        // on top of the two a message and its tool result take.
        let readings = AGENTS as u64 * (load.elapsed.as_secs() + 2);
        let ceiling = AGENTS as u64 * (2 * MESSAGES + 10) + readings;
        assert!(
            load.commits <= ceiling,
            "{label}: {} commits, at most {ceiling}",
            load.commits
        );
        // Each output is written once, compressed, outside the event rows.
        let outputs = (AGENTS * TOOL_OUTPUT_BYTES) as u64 * MESSAGES;
        assert!(
            load.stats.total_bytes < outputs / 2,
            "{label}: {} payload bytes for {outputs} of tool output",
            load.stats.total_bytes
        );
    }
}
