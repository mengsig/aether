//! One-hour prompt-cache writes priced through the Claude parser and runtime.
use super::*;
use aether_core::model::{CostBasis, Usage};
use aether_engine::claude_code::{ParseState, parse_line};
use serde_json::{Value, json};

const UNPRICED_HAIKU: &str = r#"
[[providers.claude-code.add_models]]
id = "claude-haiku-5-5"
aliases = ["haiku"]
[providers.claude-code.add_models.capabilities]
context_window = 200000
max_output_tokens = 128000
"#;

/// Claude Code 2.1.280's usage for one Opus 5.5 request, every write on the 1-hour tier.
/// Its own receipt for these counters was `total_cost_usd: 0.1193228`.
fn one_hour_usage() -> Value {
    json!({
        "input_tokens": 2,
        "cache_creation_input_tokens": 14626,
        "cache_read_input_tokens": 10234,
        "output_tokens": 13,
        "cache_creation": {"ephemeral_1h_input_tokens": 14626, "ephemeral_5m_input_tokens": 0}
    })
}

/// Replays raw stream-json lines through the real Claude parser.
struct NativeLinesEngine {
    lines: Vec<Value>,
}

#[async_trait::async_trait]
impl Engine for NativeLinesEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "native-cache-ttl-pricing/1".into()
    }

    async fn run(
        &self,
        _turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (tx, rx) = mpsc::channel(64);
        let mut state = ParseState::default();
        for line in &self.lines {
            for event in parse_line(&line.to_string(), &mut state) {
                tx.send(event).await.expect("fixture channel capacity");
            }
        }
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

/// One streamed request carrying `usage`, then a closing `result` line.
fn turn_lines(usage: Value, result: Value) -> Vec<Value> {
    vec![
        json!({"type":"stream_event","event":{"type":"message_start",
            "message":{"id":"msg_1h","usage":usage}}}),
        json!({"type":"assistant","message":{"id":"msg_1h",
            "content":[{"type":"text","text":"done"}],"usage":usage}}),
        json!({"type":"stream_event","event":{"type":"message_delta",
            "usage":{"output_tokens":usage["output_tokens"]}}}),
        json!({"type":"stream_event","event":{"type":"message_stop"}}),
        result,
    ]
}

fn result_line(usage: Value, total_cost_usd: Option<f64>) -> Value {
    let mut line = json!({"type":"result","subtype":"success","result":"done","usage":usage});
    if let Some(cost) = total_cost_usd {
        line["total_cost_usd"] = json!(cost);
    }
    line
}

async fn settled_usage(h: &Harness) -> Usage {
    let id = chat_agent(h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("price usage")], Actor::user())
        .unwrap();
    let usage = wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns == 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await
    .stats
    .usage;
    assert_eq!(usage.cache_write_tokens, 14626, "{usage:?}");
    usage
}

fn per_mtok(tokens: f64, rate: f64) -> f64 {
    tokens / 1_000_000.0 * rate
}

#[tokio::test]
async fn one_hour_cache_writes_bill_at_the_one_hour_rate() {
    let engine = NativeLinesEngine {
        lines: turn_lines(one_hour_usage(), result_line(one_hour_usage(), None)),
    };
    let h = harness_with_engine_config(Arc::new(engine), |config| {
        config.default_model = Some(ModelRef::new("claude-code", "opus"));
    })
    .await;
    let usage = settled_usage(&h).await;
    // Opus 5.5: $4 input, $8 1-hour write, $0.20 read, $20 output.
    let expected = per_mtok(2.0, 4.0)
        + per_mtok(14626.0, 8.0)
        + per_mtok(10234.0, 0.20)
        + per_mtok(13.0, 20.0);
    assert!((expected - 0.1193228).abs() < 1e-12, "{expected}");
    assert!(
        (usage.cost_usd - expected).abs() < 1e-12,
        "priced ${:.7}, Claude Code billed ${expected:.7}",
        usage.cost_usd
    );
    assert!(!usage.cost_complete);
    assert_eq!(
        usage.cost_gap,
        Some(aether_core::model::CostGap::Unreported)
    );
    assert_eq!(usage.cost_basis, CostBasis::ListPrice);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn one_hour_writes_missing_from_a_short_receipt_bill_at_the_one_hour_rate() {
    let mut short = one_hour_usage();
    short["cache_creation_input_tokens"] = json!(0);
    short["cache_creation"] =
        json!({"ephemeral_1h_input_tokens": 0, "ephemeral_5m_input_tokens": 0});
    let engine = NativeLinesEngine {
        lines: turn_lines(one_hour_usage(), result_line(short, Some(0.001))),
    };
    let h = harness_with_engine(Arc::new(engine), Arc::default(), Arc::default()).await;
    let usage = settled_usage(&h).await;
    // Haiku 5.5 bills a 1-hour write at $0.20, twice its $0.10 input.
    let expected = 0.001 + per_mtok(14626.0, 0.20);
    assert!(
        (usage.cost_usd - expected).abs() < 1e-12,
        "priced ${:.8}, expected ${expected:.8}",
        usage.cost_usd
    );
    assert!(usage.cost_complete);
    assert_eq!(usage.cost_basis, CostBasis::Mixed);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn one_hour_writes_estimate_at_the_one_hour_rate() {
    let engine = NativeLinesEngine {
        lines: turn_lines(one_hour_usage(), result_line(one_hour_usage(), None)),
    };
    let h = harness_with_engine_and_providers_toml(
        Arc::new(engine),
        Arc::default(),
        Arc::default(),
        Some(UNPRICED_HAIKU),
    )
    .await;
    let usage = settled_usage(&h).await;
    let expected = per_mtok(2.0, 0.10)
        + per_mtok(14626.0, 0.20)
        + per_mtok(10234.0, 0.01)
        + per_mtok(13.0, 0.50);
    let estimate = usage
        .estimated_cost_usd
        .expect("an API-equivalent estimate");
    assert!(
        (estimate - expected).abs() < 1e-12,
        "estimated ${estimate:.8}, expected ${expected:.8}"
    );
    assert_eq!(usage.cost_usd, 0.0);
    assert!(!usage.cost_complete);
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn a_split_reported_after_its_writes_still_bills_the_one_hour_rate() {
    let mut unsplit = one_hour_usage();
    unsplit.as_object_mut().unwrap().remove("cache_creation");
    let mut lines = turn_lines(one_hour_usage(), result_line(one_hour_usage(), None));
    lines[0]["event"]["message"]["usage"] = unsplit;
    let engine = NativeLinesEngine { lines };
    let h = harness_with_engine_config(Arc::new(engine), |config| {
        config.default_model = Some(ModelRef::new("claude-code", "opus"));
    })
    .await;
    let usage = settled_usage(&h).await;
    assert!(
        (usage.cost_usd - 0.1193228).abs() < 1e-12,
        "priced ${:.7}, Claude Code billed $0.1193228",
        usage.cost_usd
    );
    assert_eq!(usage.cost_basis, CostBasis::ListPrice);
    h.runtime.shutdown().await;
}
