//! Provider quota windows and tool-duration events.
use super::*;
#[tokio::test]
async fn provider_usage_keeps_every_reported_window_in_health_and_events() {
    let h = harness(Reply::Quota).await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("Report usage")], Actor::user())
        .unwrap();
    wait_for(h.store.as_ref(), id, |agent| {
        agent.stats.turns > 0 && agent.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    let health =
        serde_json::to_value(h.runtime.provider_health_for("claude-code").unwrap()).unwrap();
    assert_eq!(
        health["quota"]["used_percent"], 42,
        "the scheduling window stays unchanged"
    );
    assert_eq!(
        health["usage"]["windows"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        2,
        "both subscription windows must be available to /usage"
    );
    assert!(health["usage"]["observed_at"].is_string());
    let events = h.store.events(0, 1000, &EventFilter::default()).unwrap();
    let event = events
        .iter()
        .find(|event| event.type_name() == "provider.quota")
        .unwrap();
    let event = serde_json::to_value(event).unwrap();
    assert_eq!(
        event["kind"]["usage"], health["usage"],
        "health and durable events retain the same report"
    );
    assert_headroom(&h.runtime, &health);
    h.runtime.shutdown().await;
}

fn assert_headroom(runtime: &Runtime, health: &serde_json::Value) {
    let usage = runtime.provider_usage();
    let headroom = &usage["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["provider"] == "claude-code")
        .unwrap()["headroom"];
    assert_eq!(headroom["windows"][0]["remaining_percent"], 58);
    assert_eq!(headroom["windows"][1]["remaining_percent"], 27);
    assert!(headroom["windows"][0]["resets_at"].is_string());
    assert!(
        headroom["windows"][0]["resets_in_seconds"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(headroom["observed_at"], health["usage"]["observed_at"]);
    assert_eq!(headroom["stale"], false);
}

#[tokio::test]
async fn native_tool_duration_measures_the_observed_execution_interval() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let systems = Arc::new(Mutex::new(Vec::new()));
    let engine = Arc::new(ScriptedEngine {
        reply: Reply::ToolRun,
        prompts: prompts.clone(),
        systems: systems.clone(),
        costs: Arc::new(Mutex::new(Default::default())),
        tool_gate: Some(gate.clone()),
    });
    let h = harness_with_engine(engine, prompts, systems).await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("Inspect files")], Actor::user())
        .unwrap();
    wait_for(h.store.as_ref(), id, |_| {
        h.store
            .events(0, 1000, &EventFilter::default())
            .unwrap()
            .iter()
            .any(|event| matches!(event.kind, EventKind::ToolStarted { .. }))
    })
    .await;
    // Release only after the runtime recorded the start, so queueing the
    // scripted events cannot erase the interval being measured.
    tokio::time::sleep(Duration::from_millis(30)).await;
    gate.notify_one();
    wait_for(h.store.as_ref(), id, |agent| {
        agent.stats.turns > 0 && agent.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    let events = h.store.events(0, 1000, &EventFilter::default()).unwrap();
    let duration = events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::ToolSucceeded { outcome, .. } => Some(outcome.duration_ms),
            _ => None,
        })
        .expect("native result");
    assert!(
        duration >= 20,
        "native execution was recorded as {duration}ms"
    );
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn usage_includes_custom_enabled_providers_and_honors_freshness_config() {
    let h = custom_usage_harness().await;
    let id = chat_agent(&h);
    h.runtime
        .send_message(id, vec![ContentBlock::text("Report usage")], Actor::user())
        .unwrap();
    wait_for(h.store.as_ref(), id, |agent| {
        agent.stats.turns > 0 && agent.status.kind() == AgentStatusKind::Waiting
    })
    .await;
    let usage = h.runtime.provider_usage();
    let providers = usage["providers"].as_array().unwrap();
    let extra = providers
        .iter()
        .find(|row| row["provider"] == "extra-subscription")
        .unwrap();
    assert_eq!(
        extra["headroom"],
        serde_json::json!({
            "windows": [], "observed_at": null, "age_seconds": null, "stale": true
        })
    );
    assert!(!providers.iter().any(|row| row["provider"] == "codex"));
    let claude = providers
        .iter()
        .find(|row| row["provider"] == "claude-code")
        .unwrap();
    assert_eq!(claude["headroom"]["stale"], true);
    assert_eq!(claude["headroom"]["windows"].as_array().unwrap().len(), 2);
    h.runtime.shutdown().await;
}

async fn custom_usage_harness() -> Harness {
    let engine = Arc::new(ScriptedEngine {
        reply: Reply::Quota,
        prompts: Arc::default(),
        systems: Arc::default(),
        costs: Arc::default(),
        tool_gate: None,
    });
    build_harness(
        engine,
        Arc::default(),
        Arc::default(),
        Some(
            r#"
[providers.extra-subscription]
name = "Extra subscription"
protocol = "openai-chat-completions"
base_url = "http://localhost:8080/v1"
auth = { none = true }
[providers.codex]
enabled = false
"#,
        ),
        Some("[providers]\nusage_stale_after = 0"),
        None,
        |_| {},
    )
    .await
}
