//! Unusable Claude receipts preserve the floor even when every stream token has a price.
use super::*;
use aether_core::model::{CostBasis, CostGap};
use aether_engine::claude_code::{ParseState, parse_line};

fn pricing() -> TurnPricing {
    TurnPricing {
        billed: Some(ModelCost {
            input_per_mtok: 1_000_000.0,
            output_per_mtok: 2_000_000.0,
            ..Default::default()
        }),
        estimate: None,
    }
}

fn closing(dollars: Value) -> Value {
    json!({"type":"result", "subtype":"success", "session_id":"s",
        "total_cost_usd": dollars, "modelUsage": {},
        "usage":{"input_tokens":200,"output_tokens":10}})
}

fn stream_request() -> Value {
    json!({"type":"assistant", "message":{"id":"m", "model":"haiku",
        "content":[], "usage":{"input_tokens":100,"output_tokens":20}}})
}

fn zero_closing() -> Value {
    let mut receipt = closing(json!(0.0));
    receipt["usage"] = json!({"input_tokens":0,"output_tokens":0});
    receipt
}

#[tokio::test]
async fn model_less_stream_usage_survives_an_all_zero_receipt() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let mut ctx = started_agent_ctx(&runtime).await;
    let snapshot = usage::bare_snapshot(ctx.agent.id);
    let resolved = runtime.models.resolve(&snapshot.model).unwrap();
    let mut state = StreamState::new(pricing(), resolved);
    let mut parser = ParseState::default();
    let delta = json!({"type":"stream_event","event":{"type":"message_delta",
        "usage":{"input_tokens":100,"output_tokens":20}}});
    for event in parse_line(&delta.to_string(), &mut parser) {
        handle_event(&runtime, &mut ctx, &snapshot, event, &mut state).unwrap();
    }
    assert_eq!(state.stream_usage.cost_usd, 140.0);
    let outcome = read_closing(&runtime, &mut ctx, &mut state, &mut parser, &zero_closing());
    let usage = settle_state(&runtime, &mut ctx, state, outcome);
    assert_eq!(usage.cost_usd, 140.0);
    assert_eq!((usage.input_tokens, usage.output_tokens), (100, 20));
    assert!(!usage.cost_complete);
    assert_eq!(usage.cost_gap, Some(CostGap::Unreported));
    runtime.shutdown().await;
}

#[tokio::test]
async fn an_all_zero_receipt_without_work_keeps_pre_pr_settlement() {
    let events = parse_line(&zero_closing().to_string(), &mut ParseState::default());
    assert!(
        matches!(&events[..], [EngineEvent::ProtocolNote(_), EngineEvent::Done(done)]
        if done.cost_usd.is_none() && done.cost_usage.is_none())
    );
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let usage = settle(&runtime, ParseState::default(), &[zero_closing()], false).await;
    assert_eq!(usage, Usage::default());
    runtime.shutdown().await;
}

#[tokio::test]
async fn runtime_work_evidence_still_marks_a_gap_without_usage() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    for counters in [Value::Null, json!({"input_tokens":"unknown"}), json!({})] {
        let mut receipt = zero_closing();
        receipt["result"] = json!("Built.");
        receipt["usage"] = counters;
        let events = parse_line(&receipt.to_string(), &mut ParseState::default());
        assert!(matches!(
            &events[..],
            [EngineEvent::ProtocolNote(_), EngineEvent::Done(_)]
        ));
        let usage = settle(&runtime, ParseState::default(), &[receipt], false).await;
        assert_eq!(usage.total_tokens(), 0);
        assert_eq!(usage.cost_usd, 0.0);
        assert!(!usage.cost_complete);
        assert_eq!(usage.cost_gap, Some(CostGap::Unreported));
    }
    runtime.shutdown().await;
}

fn read_closing(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    state: &mut StreamState,
    parser: &mut ParseState,
    line: &Value,
) -> EngineOutcome {
    assert_eq!(line["type"], "result");
    let snapshot = usage::bare_snapshot(ctx.agent.id);
    let mut outcome = None;
    for event in parse_line(&line.to_string(), parser) {
        match event {
            EngineEvent::Done(done) => outcome = Some(done),
            event => handle_event(runtime, ctx, &snapshot, event, state).unwrap(),
        }
    }
    let outcome = outcome.expect("a closing result must emit Done");
    assert!(outcome.closing_accounting);
    outcome
}

async fn settle(
    runtime: &Arc<Runtime>,
    mut parser: ParseState,
    lines: &[Value],
    streamed: bool,
) -> Usage {
    assert!(!lines.is_empty());
    let mut ctx = started_agent_ctx(runtime).await;
    let snapshot = usage::bare_snapshot(ctx.agent.id);
    let resolved = runtime.models.resolve(&snapshot.model).unwrap();
    let mut state = StreamState::new(pricing(), resolved);
    if streamed {
        for event in parse_line(&stream_request().to_string(), &mut parser) {
            handle_event(runtime, &mut ctx, &snapshot, event, &mut state).unwrap();
        }
        assert_eq!(state.stream_usage.cost_usd, 140.0);
    }
    let mut outcome = EngineOutcome::default();
    for line in lines {
        outcome = read_closing(runtime, &mut ctx, &mut state, &mut parser, line);
    }
    settle_state(runtime, &mut ctx, state, outcome)
}

fn settle_state(
    runtime: &Arc<Runtime>,
    ctx: &mut TurnCtx,
    state: StreamState,
    outcome: EngineOutcome,
) -> Usage {
    assert_eq!(state.usage_seen, state.usage_before.is_some());
    assert!(outcome.closing_accounting);
    let run = DriveResult {
        outcome,
        priced: state.priced,
        usage_seen: state.usage_seen,
        usage_before: state.usage_before,
        stream_usage: state.stream_usage,
        request_usage_seen: state.request_usage_seen,
        pricing: state.pricing,
        ..super::super::execution::test_run()
    };
    super::super::execution::settle_usage(runtime, ctx, &run).unwrap();
    assert_eq!(
        runtime
            .store
            .agent(ctx.agent.id)
            .unwrap()
            .unwrap()
            .stats
            .usage,
        ctx.agent.stats.usage
    );
    ctx.turn_usage
}

#[tokio::test]
async fn unusable_receipts_price_unstreamed_closing_tokens_into_a_floor() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    for dollars in [Value::Null, json!(-1.0), json!(0.0)] {
        let mut receipt = closing(dollars);
        if receipt["total_cost_usd"].is_null() {
            receipt.as_object_mut().unwrap().remove("total_cost_usd");
        }
        let usage = settle(&runtime, ParseState::default(), &[receipt.clone()], true).await;
        assert_eq!(usage.cost_usd, 240.0, "receipt: {receipt}");
        assert_eq!((usage.input_tokens, usage.output_tokens), (200, 20));
        assert!(!usage.cost_complete, "receipt: {receipt}");
        assert_eq!(usage.cost_gap, Some(CostGap::Unreported));
        assert_eq!(usage.cost_basis, CostBasis::ListPrice);
    }
    runtime.shutdown().await;
}

#[tokio::test]
async fn unusable_receipt_without_streamed_tokens_still_prices_a_floor() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let usage = settle(
        &runtime,
        ParseState::default(),
        &[closing(json!(0.0))],
        false,
    )
    .await;
    assert_eq!(usage.cost_usd, 220.0);
    assert_eq!(usage.total_tokens(), 210);
    assert!(!usage.cost_complete);
    assert_eq!(usage.cost_gap, Some(CostGap::Unreported));
    runtime.shutdown().await;
}

#[tokio::test]
async fn later_cumulative_dollars_can_cover_an_earlier_unusable_receipt() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    for dollars in [Value::Null, json!(-1.0), json!(0.0)] {
        let mut recovered = closing(json!(250.0));
        recovered["usage"]["output_tokens"] = json!(20);
        let usage = settle(
            &runtime,
            ParseState::default(),
            &[closing(dollars), recovered],
            true,
        )
        .await;
        assert_eq!(usage.cost_usd, 250.0);
        assert!(usage.cost_complete);
        assert_eq!(usage.cost_gap, None);
    }
    runtime.shutdown().await;
}

#[tokio::test]
async fn codex_list_price_stream_without_provider_dollars_stays_complete() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let mut ctx = started_agent_ctx(&runtime).await;
    let snapshot = usage::bare_snapshot(ctx.agent.id);
    let resolved = runtime.models.resolve(&snapshot.model).unwrap();
    let mut state = StreamState::new(pricing(), resolved);
    let mut parser = aether_engine::codex::ParseState::default();
    let receipt = json!({"type":"turn.completed", "usage":{
        "input_tokens":100,"cached_input_tokens":0,"output_tokens":20}});
    let mut outcome = None;
    for event in aether_engine::codex::parse_line(&receipt.to_string(), &mut parser) {
        match event {
            EngineEvent::Done(done) => outcome = Some(done),
            event => handle_event(&runtime, &mut ctx, &snapshot, event, &mut state).unwrap(),
        }
    }
    let usage = settle_state(&runtime, &mut ctx, state, outcome.unwrap());
    assert_eq!(usage.cost_usd, 140.0);
    assert!(usage.cost_complete);
    assert_eq!(usage.cost_gap, None);
    assert_eq!(usage.cost_basis, CostBasis::ListPrice);
    runtime.shutdown().await;
}

fn resumed_parser(prior: Usage) -> ParseState {
    assert!(prior.cost_usd.is_finite() && prior.cost_usd >= 0.0);
    assert!(!prior.cost_complete || (prior.total_tokens() == 0 && prior.cost_usd == 0.0));
    ParseState::for_turn(&EngineTurn {
        redactor: Default::default(),
        prompt: String::new(),
        images: Vec::new(),
        system_prompt: String::new(),
        session: Some("s".into()),
        session_usage: prior,
        session_model_usage: None,
        new_session_id: String::new(),
        session_recorder: None,
        model: None,
        effort: None,
        cwd: std::path::PathBuf::from("."),
        extra_dirs: Vec::new(),
        summary_only: false,
        compact_session: false,
        auto_compact_tokens: None,
        lean_host_context: false,
        network_access: false,
        bypass_permissions: false,
        hidden_env: Vec::new(),
        tool_hidden_env: Vec::new(),
        temp_dir: None,
        build_env: Vec::new(),
        mcp: None,
    })
}

#[tokio::test]
async fn a_session_after_only_a_cost_gap_can_price_unstreamed_closing_tokens() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    let mut ctx = started_agent_ctx(&runtime).await;
    let snapshot = usage::bare_snapshot(ctx.agent.id);
    let model = aether_core::model::ModelRef::new("claude-code", "default");
    let resolved = runtime.models.resolve(&model).unwrap();
    let mut state = StreamState::new(TurnPricing::of(&runtime, &resolved), resolved);
    assert!(state.pricing.billed.is_none());
    let mut parser = ParseState::default();
    read_closing(
        &runtime,
        &mut ctx,
        &mut state,
        &mut parser,
        &closing(Value::Null),
    );
    let before = state.usage_before;
    assert!(state.usage_seen && before.is_some());
    handle_event(
        &runtime,
        &mut ctx,
        &snapshot,
        EngineEvent::Session {
            id: "s".into(),
            model: Some("claude-fable-5-1".into()),
            tools: Vec::new(),
        },
        &mut state,
    )
    .unwrap();
    let rate = state
        .pricing
        .billed
        .expect("a gap alone must not block session pricing");
    assert_eq!(state.usage_before, before);
    let outcome = read_closing(
        &runtime,
        &mut ctx,
        &mut state,
        &mut parser,
        &closing(Value::Null),
    );
    let expected = rate.cost_of(&outcome.usage);
    let usage = settle_state(&runtime, &mut ctx, state, outcome);
    assert_eq!(usage.cost_usd, expected);
    assert!(expected > 0.0);
    assert!(!usage.cost_complete);
    assert_eq!(usage.cost_gap, Some(CostGap::Unreported));
    runtime.shutdown().await;
}

#[tokio::test]
async fn missing_baseline_keeps_later_receipts_as_floors_in_the_same_run() {
    let (runtime, _dir) = test_runtime(|_| {}).await;
    for prior in [
        Usage::default(),
        Usage {
            input_tokens: 100,
            cost_usd: 50.0,
            cost_complete: false,
            cost_gap: Some(CostGap::Unreported),
            ..Default::default()
        },
    ] {
        for streamed in [false, true] {
            let usage = settle(
                &runtime,
                resumed_parser(prior),
                &[closing(json!(250.0)), closing(json!(300.0))],
                streamed,
            )
            .await;
            assert_eq!(usage.cost_usd, if streamed { 240.0 } else { 220.0 });
            assert!(!usage.cost_complete);
            assert_eq!(usage.cost_gap, Some(CostGap::Unreported));
        }
    }
    runtime.shutdown().await;
}
