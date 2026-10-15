//! Complete-request tier pricing through the native parser and runtime driver.
use super::*;
use aether_core::model::Usage;
use aether_engine::claude_code::{ParseState, parse_line};
use serde_json::json;

const UNPRICED_HAIKU: &str = r#"
[[providers.claude-code.add_models]]
id = "claude-haiku-5-5"
aliases = ["haiku"]
[providers.claude-code.add_models.capabilities]
context_window = 200000
max_output_tokens = 128000
"#;

struct NativeRequestEngine {
    requests: Vec<Usage>,
    closing_cost: Option<f64>,
    closing_extra: Usage,
}

#[async_trait::async_trait]
impl Engine for NativeRequestEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::CLAUDE_CODE
    }

    fn adapter(&self) -> String {
        "native-request-pricing/1".into()
    }

    async fn run(
        &self,
        _turn: EngineTurn,
        _cancel: CancellationToken,
    ) -> Result<EngineRun, EngineError> {
        let (tx, rx) = mpsc::channel(64);
        let mut state = ParseState::default();
        let mut total = Usage::default();
        for (index, usage) in self.requests.iter().enumerate() {
            total += *usage;
            for line in [
                json!({"type":"stream_event","event":{"type":"message_start",
                    "message":{"id":format!("request-{index}"),"usage":{
                        "input_tokens":usage.input_tokens,
                        "cache_read_input_tokens":usage.cache_read_tokens,
                        "cache_creation_input_tokens":usage.cache_write_tokens}}}}),
                json!({"type":"assistant","message":{"id":format!("request-{index}"),
                    "content":[{"type":"text","text":"done"}]}}),
                json!({"type":"stream_event","event":{"type":"message_delta",
                    "usage":{"output_tokens":usage.output_tokens}}}),
                json!({"type":"stream_event","event":{"type":"message_stop"}}),
            ] {
                for event in parse_line(&line.to_string(), &mut state) {
                    tx.send(event).await.expect("fixture channel capacity");
                }
            }
        }
        total += self.closing_extra;
        tx.send(EngineEvent::Done(EngineOutcome {
            closing_accounting: true,
            text: Some("done".into()),
            usage: total,
            cost_usd: self.closing_cost,
            ..Default::default()
        }))
        .await
        .expect("fixture channel capacity");
        Ok(EngineRun {
            events: rx,
            input: None,
            decisions: None,
            stop: None,
        })
    }
}

fn request(prompt: u64) -> Usage {
    Usage {
        input_tokens: 10,
        cache_read_tokens: prompt - 20,
        cache_write_tokens: 10,
        output_tokens: 2_000,
        ..Default::default()
    }
}

fn haiku_price() -> aether_core::model::ModelCost {
    aether_model::Registry::builtin()
        .resolve(&ModelRef::new("claude-code", "haiku"))
        .unwrap()
        .model
        .capabilities
        .cost
        .unwrap()
}

fn streamed_totals(h: &Harness, id: AgentId) -> Vec<Usage> {
    h.store
        .events(0, 1000, &EventFilter::own_events(id))
        .unwrap()
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::AgentUsageUpdated { total, .. } => Some(total),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn native_request_pricing_bills_late_output_and_each_request_once() {
    let rate = haiku_price();
    for requests in [
        vec![request(100_001)],
        vec![request(100_000)],
        vec![request(60_000), request(60_000)],
        vec![request(100_001), request(60_000)],
    ] {
        let expected: f64 = requests.iter().map(|usage| rate.cost_of(usage)).sum();
        let h = harness_with_engine(
            Arc::new(NativeRequestEngine {
                requests: requests.clone(),
                closing_cost: None,
                closing_extra: Usage::default(),
            }),
            Arc::default(),
            Arc::default(),
        )
        .await;
        let id = chat_agent(&h);
        for turn in 1..=2 {
            h.runtime
                .send_message(id, vec![ContentBlock::text("price usage")], Actor::user())
                .unwrap();
            let agent = wait_for(h.store.as_ref(), id, |a| {
                a.stats.turns == turn && a.status.kind() == AgentStatusKind::Waiting
            })
            .await;
            let usage = agent.stats.usage;
            assert!(
                (usage.cost_usd - expected * turn as f64).abs() < 1e-12,
                "{requests:?}: {usage:?}"
            );
            assert!(usage.cost_complete);
            assert_eq!(usage.estimated_cost_usd, None);
            assert_eq!(usage.cost_basis, aether_core::model::CostBasis::ListPrice);
            assert_eq!(
                usage.output_tokens,
                requests.len() as u64 * 2_000 * turn as u64
            );
        }
        let updates = streamed_totals(&h, id);
        let mut input_only = requests[0];
        input_only.output_tokens = 0;
        assert!((updates[0].cost_usd - rate.cost_of(&input_only)).abs() < 1e-12);
        assert!((updates[1].cost_usd - rate.cost_of(&requests[0])).abs() < 1e-12);
        let settled = 2 * requests.len();
        assert_eq!(
            updates[settled],
            updates[settled - 1],
            "reconciliation charges nothing twice"
        );
        h.runtime.shutdown().await;
    }
}

#[tokio::test]
async fn native_request_pricing_preserves_request_estimates_at_reconciliation() {
    let requests = vec![request(60_000), request(60_000), request(100_001)];
    let expected: f64 = requests
        .iter()
        .map(|usage| haiku_price().cost_of(usage))
        .sum();
    let h = harness_with_engine_and_providers_toml(
        Arc::new(NativeRequestEngine {
            requests,
            closing_cost: None,
            closing_extra: Usage::default(),
        }),
        Arc::default(),
        Arc::default(),
        Some(UNPRICED_HAIKU),
    )
    .await;
    let id = chat_agent(&h);
    for turn in 1..=2 {
        h.runtime
            .send_message(
                id,
                vec![ContentBlock::text("estimate usage")],
                Actor::user(),
            )
            .unwrap();
        let agent = wait_for(h.store.as_ref(), id, |a| {
            a.stats.turns == turn && a.status.kind() == AgentStatusKind::Waiting
        })
        .await;
        let usage = agent.stats.usage;
        assert_eq!(usage.cost_usd, 0.0);
        assert!(!usage.cost_complete);
        assert!((usage.estimated_cost_usd.unwrap() - expected * turn as f64).abs() < 1e-12);
        assert_eq!(usage.cost_basis, aether_core::model::CostBasis::ListPrice);
    }
    let updates = streamed_totals(&h, id);
    assert_eq!(
        updates[6], updates[5],
        "request estimates survive closing totals"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn native_request_pricing_replaces_streamed_estimates_with_a_provider_receipt() {
    let h = harness_with_engine_and_providers_toml(
        Arc::new(NativeRequestEngine {
            requests: vec![request(100_001)],
            closing_cost: Some(0.125),
            closing_extra: Usage::default(),
        }),
        Arc::default(),
        Arc::default(),
        Some(UNPRICED_HAIKU),
    )
    .await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("price usage")], Actor::user())
        .unwrap();
    let usage = wait_for(h.store.as_ref(), id, |a| {
        a.stats.turns == 1 && a.status.kind() == AgentStatusKind::Waiting
    })
    .await
    .stats
    .usage;
    assert_eq!(usage.cost_usd, 0.125);
    assert!(usage.cost_complete);
    assert_eq!(usage.estimated_cost_usd, None);
    assert_eq!(
        usage.cost_basis,
        aether_core::model::CostBasis::ProviderReported
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn native_request_pricing_estimates_unstreamed_closing_tokens_at_base_rates() {
    let requests = vec![request(60_000), request(100_001)];
    let streamed_estimate: f64 = requests
        .iter()
        .map(|usage| haiku_price().cost_of(usage))
        .sum();
    let extra = Usage {
        input_tokens: 20_000,
        output_tokens: 3_000,
        cache_read_tokens: 60_000,
        cache_write_tokens: 21_000,
        ..Default::default()
    };
    // Unassigned prompt tokens exceed the tier threshold, but have no request boundary.
    let base_estimate = 0.002 + 0.0015 + 0.0006 + 0.002625;
    let h = harness_with_engine_and_providers_toml(
        Arc::new(NativeRequestEngine {
            requests,
            closing_cost: None,
            closing_extra: extra,
        }),
        Arc::default(),
        Arc::default(),
        Some(UNPRICED_HAIKU),
    )
    .await;
    let id = chat_agent(&h);
    for turn in 1..=2 {
        h.runtime
            .send_message(
                id,
                vec![ContentBlock::text("estimate closing usage")],
                Actor::user(),
            )
            .unwrap();
        let usage = wait_for(h.store.as_ref(), id, |a| {
            a.stats.turns == turn && a.status.kind() == AgentStatusKind::Waiting
        })
        .await
        .stats
        .usage;
        assert_eq!(usage.cost_usd, 0.0);
        assert!(!usage.cost_complete);
        assert_eq!(usage.cost_basis, aether_core::model::CostBasis::ListPrice);
        assert!(
            (usage.estimated_cost_usd.unwrap() - (streamed_estimate + base_estimate) * turn as f64)
                .abs()
                < 1e-12
        );
        assert_eq!(usage.input_tokens, 20_020 * turn as u64);
        assert_eq!(usage.output_tokens, 7_000 * turn as u64);
        assert_eq!(usage.cache_read_tokens, 219_961 * turn as u64);
        assert_eq!(usage.cache_write_tokens, 21_020 * turn as u64);
    }
    let updates = streamed_totals(&h, id);
    assert!((updates[3].estimated_cost_usd.unwrap() - streamed_estimate).abs() < 1e-12);
    assert!(
        (updates[4].estimated_cost_usd.unwrap()
            - updates[3].estimated_cost_usd.unwrap()
            - base_estimate)
            .abs()
            < 1e-12
    );
    h.runtime.shutdown().await;
}
