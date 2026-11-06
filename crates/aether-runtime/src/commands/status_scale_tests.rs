//! `status` on a store the size of a long-lived daemon's: five million events,
//! most of them one orchestrator run five days old. Run by hand, in release:
//! `AETHER_STATUS_BENCH_DIR=/var/tmp/status-bench cargo test --release -p aether-runtime
//! status_at_scale -- --ignored --nocapture`. The store is built once and reused.

use super::*;
use crate::{CreateAgentSpec, RuntimeConfig};
use aether_core::agent::WaitCondition;
use aether_core::event::{Actor, EventDraft};
use aether_core::id::{EventId, ToolCallId};
use aether_core::model::{ModelRef, Usage};
use aether_store::{ImportedEvent, Mutation, SqliteStore, Store};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing_subscriber::Layer;

const EVENTS: usize = 5_000_000;
const RUN_MEMBERS: usize = 400;
const LIVE_MEMBERS: usize = 20;
const OLD_AGENTS: usize = 2_000;
/// One event in this many of the big run is a usage receipt.
const RECEIPT_EVERY: usize = 2_000;
const BATCH: usize = 20_000;
const SAMPLES: usize = 40;

fn usage(cost: f64) -> Usage {
    Usage {
        input_tokens: 1_000,
        output_tokens: 100,
        cost_usd: cost,
        cost_complete: true,
        ..Default::default()
    }
}

/// `count` copies of `template` in `run`, created `age` ago, the first `live` of them waiting.
fn members(
    template: &Agent,
    run: RunId,
    count: usize,
    live: usize,
    age: chrono::Duration,
) -> Vec<Agent> {
    let at = now() - age;
    (0..count)
        .map(|i| {
            let mut agent = template.clone();
            agent.id = AgentId::new();
            agent.run = run;
            agent.parent = None;
            agent.created_at = at + chrono::Duration::milliseconds(i as i64);
            agent.stats.usage = usage(0.5 + i as f64 * 0.01);
            agent.status = if i < live {
                AgentStatus::Waiting {
                    condition: WaitCondition::Inbox,
                }
            } else {
                AgentStatus::Completed { summary: None }
            };
            agent
        })
        .collect()
}

/// The `i`th event of the big run: mostly filler, a usage receipt now and then.
fn event(i: usize, agent: &Agent, started: Timestamp, span_ms: i64) -> ImportedEvent {
    let at = started + chrono::Duration::milliseconds(span_ms * i as i64 / EVENTS as i64);
    let kind = if i.is_multiple_of(RECEIPT_EVERY) {
        let total = 1.0 + i as f64 / EVENTS as f64;
        EventKind::AgentUsageUpdated {
            total: usage(total),
            turn_usage: usage(0.01),
        }
    } else if i.is_multiple_of(2) {
        EventKind::AgentStarted {
            turn: (i % 1000) as u32,
        }
    } else {
        EventKind::ToolDenied {
            call: ToolCallId::new(),
            reason: "denied by a synthetic rule so the body has the size of a real one".repeat(3),
        }
    };
    ImportedEvent {
        id: EventId::new(),
        draft: EventDraft::new(Actor::user(), kind)
            .agent(agent.id)
            .run(agent.run),
        at,
    }
}

fn build(runtime: &Runtime, store: &SqliteStore) {
    let (template, _) = runtime
        .create_agent(
            CreateAgentSpec {
                role: "worker".into(),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("template agent");
    let mut old = Vec::new();
    for chunk in 0..OLD_AGENTS / 5 {
        let age = chrono::Duration::days(10 + (chunk % 20) as i64);
        old.extend(members(&template, RunId::new(), 5, 0, age));
    }
    let big = members(
        &template,
        RunId::new(),
        RUN_MEMBERS,
        LIVE_MEMBERS,
        chrono::Duration::days(5),
    );
    let agents: Vec<Mutation> = old
        .iter()
        .chain(&big)
        .cloned()
        .map(Mutation::UpsertAgent)
        .collect();
    store.import(Vec::new(), agents).expect("agents");
    let started = now() - chrono::Duration::days(5);
    let span_ms = chrono::Duration::days(5).num_milliseconds();
    for batch in 0..EVENTS / BATCH {
        let events = (batch * BATCH..(batch + 1) * BATCH)
            .map(|i| event(i, &big[i % RUN_MEMBERS], started, span_ms))
            .collect();
        store.import(events, Vec::new()).expect("events");
    }
    assert!(
        store.last_seq().expect("seq") >= EVENTS as u64,
        "every event landed"
    );
}

/// Every `status section` timing the runtime logs, by section.
#[derive(Clone, Default)]
struct Sections(Arc<Mutex<Vec<(String, u64)>>>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Sections {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        #[derive(Default)]
        struct Fields(Option<String>, Option<u64>);
        impl tracing::field::Visit for Fields {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                if field.name() == "section" {
                    self.0 = Some(value.to_owned());
                }
            }
            fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
                if field.name() == "elapsed_ms" {
                    self.1 = Some(value);
                }
            }
            fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        if let Fields(Some(section), Some(ms)) = fields {
            self.0.lock().expect("sections").push((section, ms));
        }
    }
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    assert!(!sorted.is_empty(), "a percentile of nothing");
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn report(sections: &Sections, totals: &mut [u64]) {
    totals.sort_unstable();
    let mut by_section: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for (section, ms) in sections.0.lock().expect("sections").iter() {
        by_section.entry(section.clone()).or_default().push(*ms);
    }
    println!("section            p50_ms  p99_ms  max_ms");
    for (section, mut ms) in by_section {
        ms.sort_unstable();
        let max = *ms.last().expect("non-empty");
        println!(
            "{section:<18} {:>6}  {:>6}  {max:>6}",
            percentile(&ms, 0.5),
            percentile(&ms, 0.99)
        );
    }
    let max = *totals.last().expect("samples");
    println!(
        "{:<18} {:>6}  {:>6}  {max:>6}",
        "status (total)",
        percentile(totals, 0.5),
        percentile(totals, 0.99)
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "builds a five-million-event store; run by hand, in release"]
async fn status_at_scale() {
    let sections = Sections::default();
    use tracing_subscriber::layer::SubscriberExt;
    let subscriber = tracing_subscriber::registry().with(
        sections
            .clone()
            .with_filter(tracing_subscriber::filter::LevelFilter::DEBUG),
    );
    tracing::subscriber::set_global_default(subscriber).expect("one subscriber");
    let dir = std::env::var_os("AETHER_STATUS_BENCH_DIR").expect("AETHER_STATUS_BENCH_DIR");
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).expect("bench dir");
    let path = dir.join("aether.db");
    let fresh = !path.exists();
    let opened = Instant::now();
    let store = Arc::new(SqliteStore::open(&path).expect("store"));
    println!(
        "opened (and migrated) in {} ms",
        opened.elapsed().as_millis()
    );
    let runtime = Runtime::start_with_engines(
        RuntimeConfig {
            default_model: Some(ModelRef::new("anthropic", "claude-sonnet-5-5")),
            workspace_root: Some(dir.clone()),
            ..Default::default()
        },
        store.clone(),
        Arc::new(aether_model::Models::load(Some(dir.clone())).expect("models")),
        Arc::new(aether_tools::ToolRegistry::with_builtins()),
        None,
        Default::default(),
    )
    .await
    .expect("runtime");
    if fresh {
        let built = Instant::now();
        build(&runtime, &store);
        println!("built in {} s", built.elapsed().as_secs());
    }
    let warm = runtime.status().expect("warm-up status");
    assert!(
        warm.agents_live >= LIVE_MEMBERS as u32,
        "the big run's live members count"
    );
    sections.0.lock().expect("sections").clear();
    let mut totals = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        let status = runtime.status().expect("status");
        totals.push(started.elapsed().as_millis() as u64);
        assert!(!status.runs.is_empty(), "the live run is summarised");
        std::thread::sleep(Duration::from_millis(20));
    }
    report(&sections, &mut totals);
}
