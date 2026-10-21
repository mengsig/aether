//! The window a context percentage is measured against is the registry's
//! figure for the agent's model, whatever window the native CLI reports.

use super::*;
use aether_engine::claude_code::{ParseState, parse_line};
use aether_model::config::PROVIDERS_CLAUDE_1M_EXAMPLE;
use aether_tools::HarnessAction;

#[tokio::test]
async fn a_1m_model_added_in_providers_toml_measures_context_against_1m() {
    for (model, window, reported_window) in [
        ("claude-opus-5-5", 1_000_000_u64, 500_000_u64),
        ("claude-opus-5-5[1m]", 1_000_000, 500_000),
    ] {
        let config_dir = tempfile::tempdir().expect("config dir");
        std::fs::write(
            config_dir.path().join("providers.toml"),
            PROVIDERS_CLAUDE_1M_EXAMPLE,
        )
        .expect("providers.toml");
        let (runtime, _dir) = crate::test_runtime_in(config_dir, |config| {
            config.default_model = Some(ModelRef::new("claude-code", model));
        })
        .await;
        let mut ctx = started_agent_ctx(&runtime).await;
        let snapshot = usage::bare_snapshot(ctx.agent.id);
        let resolved = runtime.models.resolve(&ctx.agent.model).expect(model);
        let mut state = StreamState::new(TurnPricing::default(), resolved);
        let mut parser = ParseState::default();
        let threshold = 256_000;
        for tokens in [threshold - 1, threshold] {
            for line in context_receipt(tokens, reported_window) {
                for event in parse_line(&line.to_string(), &mut parser) {
                    handle_event(&runtime, &mut ctx, &snapshot, event, &mut state)
                        .expect("engine event");
                }
            }
            let outcome = apply_action(
                &runtime,
                &mut ctx,
                HarnessAction::InspectState,
                &mut ToolEffects::default(),
            )
            .await
            .expect("inspect_state");
            let context = &outcome.structured.expect("structured state")["context"];
            assert_eq!(context["tokens"], tokens, "{model}");
            assert_eq!(context["window"], window, "{model}");
            assert_eq!(
                context["used_percent"],
                tokens as f64 / window as f64 * 100.0,
                "{model}"
            );
            assert_eq!(
                context["at_compaction_threshold"],
                tokens == threshold,
                "{model}"
            );
        }
        runtime.shutdown().await;
    }
}

/// One Claude Code turn that used `tokens` of context and reports `window`
/// as the native window.
fn context_receipt(tokens: u64, window: u64) -> [Value; 2] {
    assert!(tokens >= 100, "fixture reserves 100 uncached input tokens");
    assert!(window > 0, "fixture reports a valid native window");
    [
        json!({"type": "assistant", "message": {
            "id": format!("msg_{tokens}"), "model": "claude-opus-5-5",
            "content": [{"type": "text", "text": "Working."}],
            "usage": {"input_tokens": 100, "cache_read_input_tokens": tokens - 100,
                "output_tokens": 1}
        }}),
        json!({"type": "result", "subtype": "success", "session_id": "test-session",
        "total_cost_usd": 1.0, "modelUsage": {"claude-opus-5-5": {
            "contextWindow": window, "inputTokens": tokens, "outputTokens": 1
        }}}),
    ]
}
