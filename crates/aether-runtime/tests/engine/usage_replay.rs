//! Replayed requests and cumulative session counters.
use super::*;
/// Replays Claude Code stream-json lines through the real adapter parser, so
/// what the runtime accounts is exactly what the CLI would have produced.
struct ReplayEngine {
    lines: Vec<String>,
}

#[async_trait::async_trait]
impl Engine for ReplayEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "replay/1".into()
    }

    async fn run(
        &self,
        _turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (tx, rx) = mpsc::channel(64);
        let lines = self.lines.clone();
        tokio::spawn(async move {
            let mut state = aether_engine::claude_code::ParseState::default();
            for line in lines {
                for event in aether_engine::claude_code::parse_line(&line, &mut state) {
                    if tx.send(event).await.is_err() {
                        return;
                    }
                }
            }
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

/// End to end: the request an operator reported, replayed through
/// the adapter the daemon runs. Its blocks each repeat the same input/cache
/// figures - 16,984 per block, which the operator saw billed three times as
/// 50,952 against a 40,000 grant. This fixture adds the `message_delta` the
/// operator's excerpt did not carry, so billed once it is 16,984 + 42 =
/// 17,025: the fixture's own figure, and affordable.
#[tokio::test]
async fn a_multi_block_request_is_billed_once_and_stays_inside_its_grant() {
    let usage = serde_json::json!({"input_tokens":10,"cache_creation_input_tokens":16_973,
        "cache_read_input_tokens":0,"output_tokens":1});
    let block = |content: serde_json::Value| {
        serde_json::json!({"type":"assistant","message":{
            "id":"msg_haiku","model":"claude-haiku-5-5","content":[content],
            "usage":usage.clone()}})
        .to_string()
    };
    let lines = vec![
        serde_json::json!({"type":"stream_event","event":{"type":"message_start",
            "message":{"id":"msg_haiku","usage":usage.clone()}}})
        .to_string(),
        block(serde_json::json!({"type":"thinking","thinking":"Read the file."})),
        block(serde_json::json!({"type":"text","text":"Reading the README."})),
        block(serde_json::json!({"type":"text","text":"Done."})),
        serde_json::json!({"type":"stream_event","event":{"type":"message_delta",
            "usage":{"output_tokens":42}}})
        .to_string(),
        serde_json::json!({"type":"stream_event","event":{"type":"message_stop"}}).to_string(),
        serde_json::json!({"type":"result","subtype":"success","is_error":false,
            "session_id":"s1","result":"Done.",
            "usage":{"input_tokens":10,"cache_creation_input_tokens":16_973,
                "cache_read_input_tokens":0,"output_tokens":42}})
        .to_string(),
    ];
    let h = harness_with_engine(
        Arc::new(ReplayEngine { lines }),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let id = budgeted_agent(&h, 40_000);
    h.runtime
        .send_message(
            id,
            vec![ContentBlock::text("Read the README")],
            Actor::user(),
        )
        .expect("send");
    let agent = wait_for(h.store.as_ref(), id, |a| a.stats.turns == 1).await;
    assert_eq!(agent.stats.usage.total_tokens(), 17_025);
    assert_eq!(
        agent.status.kind(),
        AgentStatusKind::Waiting,
        "an affordable request must not be failed: {:?}",
        agent.status
    );
    let events = h
        .store
        .events(0, 1000, &EventFilter::for_agent(id))
        .expect("events");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, EventKind::BudgetExceeded { .. })),
        "the grant was never crossed"
    );
    h.runtime.shutdown().await;
}

/// Reports a session-cumulative counter the way Codex does, and records the
/// baseline each run was handed.
struct SessionCounterEngine {
    /// The counter reported at the end of each run, oldest first.
    totals: Arc<Mutex<std::collections::VecDeque<u64>>>,
    /// What `EngineTurn::session_usage` carried into each run.
    handed: Arc<Mutex<Vec<aether_core::model::Usage>>>,
}

#[async_trait::async_trait]
impl Engine for SessionCounterEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CODEX
    }

    fn adapter(&self) -> String {
        "counter/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        self.handed.lock().unwrap().push(turn.session_usage);
        let baseline = turn.session_usage.input_tokens;
        let total = self.totals.lock().unwrap().pop_front().unwrap_or(baseline);
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            let _ = tx
                .send(EngineEvent::Session {
                    id: "thread-1".into(),
                    model: None,
                    tools: Vec::new(),
                })
                .await;
            let counter = aether_core::model::Usage {
                input_tokens: total,
                ..Default::default()
            };
            let _ = tx
                .send(EngineEvent::SessionUsage {
                    models: None,
                    session: "thread-1".into(),
                    total: counter,
                })
                .await;
            let _ = tx
                .send(EngineEvent::UsageDelta {
                    usage: aether_core::model::Usage {
                        input_tokens: total - baseline,
                        ..Default::default()
                    },
                })
                .await;
            let _ = tx
                .send(EngineEvent::Done(EngineOutcome {
                    closing_accounting: true,
                    text: Some("done".into()),
                    session: Some("thread-1".into()),
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

/// An engine that counts its *session* rather than its turn is
/// billed the increase over what the last turn on that session persisted.
/// Deriving that baseline from the run's own first report bills the previous
/// turn's last request here; carrying it across turns does not.
#[tokio::test]
async fn a_session_counter_is_billed_the_increase_over_what_was_persisted() {
    let handed = Arc::new(Mutex::new(Vec::new()));
    let h = harness_with_engine(
        Arc::new(SessionCounterEngine {
            totals: Arc::new(Mutex::new([1_000u64, 1_600].into())),
            handed: handed.clone(),
        }),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("one")], Actor::user())
        .expect("send");
    let after_one = wait_for(h.store.as_ref(), id, |a| a.stats.turns == 1).await;
    assert_eq!(after_one.stats.usage.input_tokens, 1_000);
    assert_eq!(
        after_one.engine_usage.as_ref().map(|u| u.session.as_str()),
        Some("thread-1"),
        "the reading is persisted against the session it came from"
    );
    assert_eq!(after_one.engine_session_usage().input_tokens, 1_000);

    h.runtime
        .send_message(id, vec![ContentBlock::text("two")], Actor::user())
        .expect("send");
    let after_two = wait_for(h.store.as_ref(), id, |a| a.stats.turns == 2).await;
    assert_eq!(
        handed
            .lock()
            .unwrap()
            .iter()
            .map(|u| u.input_tokens)
            .collect::<Vec<_>>(),
        vec![0, 1_000],
        "the second run resumes from what the first persisted"
    );
    assert_eq!(
        after_two.stats.usage.input_tokens, 1_600,
        "only the 600 the second turn added"
    );
    h.runtime.shutdown().await;
}

/// Replays `codex exec --json` lines through the real legacy adapter parser,
/// one scripted run per turn, starting each from the baseline the runtime
/// handed it exactly as `CodexEngine::run` does.
struct CodexExecReplayEngine {
    runs: Arc<Mutex<std::collections::VecDeque<Vec<String>>>>,
    /// The CLI version the runs came from; `None` is an unknown CLI.
    version: Option<aether_engine::EngineVersion>,
}

#[async_trait::async_trait]
impl Engine for CodexExecReplayEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CODEX
    }

    fn adapter(&self) -> String {
        "codex-exec-replay/1".into()
    }

    async fn run(
        &self,
        turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let lines = self
            .runs
            .lock()
            .unwrap()
            .pop_front()
            .expect("a scripted run");
        let mut state = aether_engine::codex::ParseState::for_turn(&turn, self.version);
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for line in lines {
                for event in aether_engine::codex::parse_line(&line, &mut state) {
                    if tx.send(event).await.is_err() {
                        return;
                    }
                }
            }
        });
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

/// One `codex exec` run on thread `th-1` that answers and reports `usage`.
fn codex_exec_run(usage: serde_json::Value) -> Vec<String> {
    vec![
        serde_json::json!({"type":"thread.started","thread_id":"th-1"}).to_string(),
        serde_json::json!({"type":"item.completed",
            "item":{"id":"m","type":"agent_message","text":"done"}})
        .to_string(),
        serde_json::json!({"type":"turn.completed","usage":usage}).to_string(),
    ]
}

/// A long-lived `model` agent whose turns replay `runs` through the exec
/// parser, as an unknown CLI's.
async fn codex_exec_agent<const N: usize>(
    runs: [Vec<String>; N],
    model: &ModelRef,
) -> (Harness, AgentId) {
    codex_exec_agent_on(None, runs, model).await
}

/// [`codex_exec_agent`] replaying the runs of a CLI at `version`.
async fn codex_exec_agent_on<const N: usize>(
    version: Option<aether_engine::EngineVersion>,
    runs: [Vec<String>; N],
    model: &ModelRef,
) -> (Harness, AgentId) {
    let h = harness_with_engine(
        Arc::new(CodexExecReplayEngine {
            runs: Arc::new(Mutex::new(runs.into())),
            version,
        }),
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    )
    .await;
    let (agent, _) = h
        .runtime
        .create_agent(
            CreateAgentSpec {
                role: "assistant".into(),
                start: true,
                model: Some(model.clone()),
                config: Some(AgentConfig {
                    long_lived: true,
                    approval: ApprovalPolicy::none(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            Actor::user(),
        )
        .expect("create");
    (h, agent.id)
}

/// The legacy `codex exec` fallback reports usage only when the turn ends,
/// and as the thread's cumulative count. Each turn must still be billed at
/// the API list price - an estimate, never a floor of $0 - and a resumed
/// thread only for what it added.
#[tokio::test]
async fn a_legacy_codex_exec_turn_is_estimated_at_the_api_list_price() {
    let runs = [
        codex_exec_run(
            serde_json::json!({"input_tokens":100_000,"cached_input_tokens":60_000,
            "output_tokens":5_000,"reasoning_output_tokens":2_000}),
        ),
        codex_exec_run(
            serde_json::json!({"input_tokens":250_000,"cached_input_tokens":180_000,
            "output_tokens":9_000,"reasoning_output_tokens":3_000}),
        ),
    ];
    let model = ModelRef::new("codex", "sol");
    let (h, agent) = codex_exec_agent(runs, &model).await;
    let registry = aether_model::Registry::builtin();
    let resolved = registry.resolve(&model).expect("codex/sol");
    assert_eq!(resolved.model.capabilities.cost, None, "a subscription");
    let rate = registry
        .api_equivalent_price(&resolved)
        .expect("codex/sol has an API twin");
    // GPT-6.1 Sol: $2 input, $0.10 cached input, $10 output per MTok.
    assert_eq!(
        (
            rate.input_per_mtok,
            rate.cache_read_per_mtok,
            rate.output_per_mtok
        ),
        (2.0, 0.10, 10.0)
    );

    h.runtime
        .send_message(agent, vec![ContentBlock::text("one")], Actor::user())
        .expect("send");
    let one = wait_for(h.store.as_ref(), agent, |a| {
        a.stats.turns == 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await
    .stats
    .usage;
    // 40k fresh input, 60k cached, 5k output with the reasoning inside it.
    let first = 0.04 * 2.0 + 0.06 * 0.10 + 0.005 * 10.0;
    let estimate = one.estimated_cost_usd.expect("an estimated turn");
    assert!((estimate - first).abs() < 1e-9, "{estimate} != {first}");
    assert_eq!(one.cost_usd, 0.0, "nobody billed these dollars");
    assert!(!one.cost_complete, "an estimate is not a billed cost");
    assert_eq!(one.reasoning_tokens, Some(2_000));

    h.runtime
        .send_message(agent, vec![ContentBlock::text("two")], Actor::user())
        .expect("send");
    let two = wait_for(h.store.as_ref(), agent, |a| {
        a.stats.turns == 2 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await
    .stats
    .usage;
    // Only what the resumed thread added: 30k fresh input, 120k cached, 4k output.
    let second = 0.03 * 2.0 + 0.12 * 0.10 + 0.004 * 10.0;
    let estimate = two.estimated_cost_usd.expect("an estimated turn");
    assert!(
        (estimate - (first + second)).abs() < 1e-9,
        "{estimate} != {}",
        first + second
    );
    assert_eq!(two.cost_gap, Some(aether_core::model::CostGap::Unpriced));
    assert_eq!(
        (two.input_tokens, two.cache_read_tokens, two.output_tokens),
        (70_000, 180_000, 9_000),
        "the thread's history is never billed twice"
    );
    h.runtime.shutdown().await;
}

/// A `turn.completed` that advances input with the cached counter left out
/// would bill every new token at the uncached rate. Its tokens still count,
/// but the turn carries no price - never one presented as exact - and the
/// ledger says why.
#[tokio::test]
async fn a_codex_exec_turn_with_an_unreadable_counter_is_not_priced_as_exact() {
    let runs = [codex_exec_run(
        serde_json::json!({"input_tokens":100_000,"output_tokens":5_000}),
    )];
    let (h, agent) = codex_exec_agent(runs, &ModelRef::new("codex", "sol")).await;
    h.runtime
        .send_message(agent, vec![ContentBlock::text("one")], Actor::user())
        .expect("send");
    let usage = wait_for(h.store.as_ref(), agent, |a| {
        a.stats.turns == 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await
    .stats
    .usage;
    assert_eq!(
        (usage.input_tokens, usage.output_tokens),
        (100_000, 5_000),
        "the tokens are still counted"
    );
    assert!(!usage.cost_complete, "an inexact reading is not a price");
    assert_eq!(
        usage.cost_gap,
        Some(aether_core::model::CostGap::Unreported)
    );
    assert_eq!(usage.cost_line(), "not reported (usage not fully reported)");
    let events = h
        .store
        .events(0, 1000, &EventFilter::for_agent(agent))
        .expect("events");
    assert!(
        events.iter().any(|e| matches!(&e.kind,
            EventKind::Custom { name, payload } if name == "engine.protocol_note"
                && payload["note"].as_str().is_some_and(|n| n.contains("cached_input_tokens")))),
        "the unread counter is on the record"
    );
    h.runtime.shutdown().await;
}

/// Sends turn `n` to `agent` and returns the agent's usage once it settles.
async fn codex_exec_turn(h: &Harness, agent: AgentId, n: u32) -> aether_core::model::Usage {
    h.runtime
        .send_message(agent, vec![ContentBlock::text("go")], Actor::user())
        .expect("send");
    wait_for(h.store.as_ref(), agent, |a| {
        a.stats.turns == n && a.status.kind() == AgentStatusKind::Waiting
    })
    .await
    .stats
    .usage
}

/// Whether `agent`'s ledger holds a protocol note naming every one of `keys`.
pub(super) fn noted(h: &Harness, agent: AgentId, keys: &[&str]) -> bool {
    let events = h
        .store
        .events(0, 1000, &EventFilter::for_agent(agent))
        .expect("events");
    events.iter().any(|e| {
        matches!(&e.kind, EventKind::Custom { name, payload }
            if name == "engine.protocol_note"
                && payload["note"]
                    .as_str()
                    .is_some_and(|n| keys.iter().all(|k| n.contains(k))))
    })
}

/// A null cache-write counter, or one left out once the thread's CLI has
/// sent it, would price the turn's cache writes from a stale reading: the
/// turn is counted, never priced as exact.
#[tokio::test]
async fn a_codex_exec_turn_with_a_stale_cache_write_counter_is_not_priced_as_exact() {
    let null = [codex_exec_run(serde_json::json!({"input_tokens":100_000,
        "cached_input_tokens":60_000,"output_tokens":5_000,
        "cache_write_input_tokens":null}))];
    let (h, agent) = codex_exec_agent(null, &ModelRef::new("codex", "sol")).await;
    let usage = codex_exec_turn(&h, agent, 1).await;
    assert_eq!(
        usage.total_tokens(),
        105_000,
        "the tokens are still counted"
    );
    assert!(!usage.cost_complete, "a null counter is not a price");
    assert!(noted(&h, agent, &["cache_write_input_tokens"]));
    h.runtime.shutdown().await;

    let omitted = [
        codex_exec_run(serde_json::json!({"input_tokens":100_000,
            "cached_input_tokens":60_000,"output_tokens":5_000,
            "cache_write_input_tokens":10_000})),
        codex_exec_run(serde_json::json!({"input_tokens":250_000,
            "cached_input_tokens":180_000,"output_tokens":9_000})),
    ];
    let (h, agent) = codex_exec_agent(omitted, &ModelRef::new("codex", "sol")).await;
    let one = codex_exec_turn(&h, agent, 1).await;
    assert_eq!(
        one.cost_gap,
        Some(aether_core::model::CostGap::Unpriced),
        "every counter read: only the subscription's missing bill"
    );
    assert!(one.estimated_cost_usd.is_some_and(|cost| cost > 0.0));
    let two = codex_exec_turn(&h, agent, 2).await;
    assert_eq!(
        two.cost_gap,
        Some(aether_core::model::CostGap::Mixed),
        "the resumed thread's CLI sent cache writes before; leaving them out is unread"
    );
    assert_eq!(
        (two.input_tokens, two.cache_read_tokens, two.output_tokens),
        (70_000, 180_000, 9_000),
        "the thread's history is never billed twice"
    );
    assert!(noted(&h, agent, &["cache_write_input_tokens"]));
    h.runtime.shutdown().await;
}

/// A `turn.completed` with no usage object read nothing: the turn cannot be
/// priced as a confident zero, and the ledger names what was missing.
#[tokio::test]
async fn a_codex_exec_turn_completed_without_usage_is_not_priced_as_exact() {
    let runs = [vec![
        serde_json::json!({"type":"thread.started","thread_id":"th-1"}).to_string(),
        serde_json::json!({"type":"turn.completed"}).to_string(),
    ]];
    let (h, agent) = codex_exec_agent(runs, &ModelRef::new("codex", "sol")).await;
    let usage = codex_exec_turn(&h, agent, 1).await;
    assert!(!usage.cost_complete, "nothing read is not a $0 price");
    assert_eq!(
        usage.cost_gap,
        Some(aether_core::model::CostGap::Unreported)
    );
    assert_eq!(usage.cost_line(), "not reported (usage not fully reported)");
    assert!(noted(
        &h,
        agent,
        &["input_tokens", "cached_input_tokens", "output_tokens"]
    ));
    h.runtime.shutdown().await;
}

/// A current CLI always sends cache writes, so a resumed thread whose first
/// run reported none cannot leave the counter out on the next: that turn's
/// cache writes are unread, never priced as zero. A CLI that predates the
/// counter still may.
#[tokio::test]
async fn a_current_codex_exec_cli_cannot_leave_out_cache_writes_it_reported_as_zero() {
    let turn_two = || {
        codex_exec_run(serde_json::json!({"input_tokens":250_000,
            "cached_input_tokens":180_000,"output_tokens":9_000,
            "reasoning_output_tokens":0}))
    };
    let runs = [
        codex_exec_run(serde_json::json!({"input_tokens":100_000,
            "cached_input_tokens":60_000,"output_tokens":5_000,
            "cache_write_input_tokens":0,"reasoning_output_tokens":0})),
        turn_two(),
    ];
    let current = aether_engine::EngineVersion::new(0, 159, 2);
    let (h, agent) = codex_exec_agent_on(Some(current), runs, &ModelRef::new("codex", "sol")).await;
    let one = codex_exec_turn(&h, agent, 1).await;
    assert_eq!(
        one.cost_gap,
        Some(aether_core::model::CostGap::Unpriced),
        "every counter read: only the subscription's missing bill"
    );
    let two = codex_exec_turn(&h, agent, 2).await;
    assert_eq!(
        two.cost_gap,
        Some(aether_core::model::CostGap::Mixed),
        "an unread cache write is not a price"
    );
    assert_eq!(
        (two.input_tokens, two.cache_read_tokens, two.output_tokens),
        (70_000, 180_000, 9_000),
        "the thread's history is never billed twice"
    );
    assert!(noted(&h, agent, &["cache_write_input_tokens"]));
    h.runtime.shutdown().await;

    let runs = [
        codex_exec_run(serde_json::json!({"input_tokens":100_000,
            "cached_input_tokens":60_000,"output_tokens":5_000,
            "reasoning_output_tokens":0})),
        turn_two(),
    ];
    let older = aether_engine::EngineVersion::new(0, 144, 6);
    let (h, agent) = codex_exec_agent_on(Some(older), runs, &ModelRef::new("codex", "sol")).await;
    codex_exec_turn(&h, agent, 1).await;
    let two = codex_exec_turn(&h, agent, 2).await;
    assert_eq!(
        two.cost_gap,
        Some(aether_core::model::CostGap::Unpriced),
        "a CLI without the counter lost nothing"
    );
    assert!(!noted(&h, agent, &["cache_write_input_tokens"]));
    h.runtime.shutdown().await;
}
