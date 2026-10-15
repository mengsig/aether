use super::*;

#[tokio::test]
async fn delegation_requires_model_and_effort_before_creating_work() {
    let mut h = harness(10).await;
    let coordinator = start_coordinator(&h, None);
    let bridge = h.ready.recv().await.expect("coordinator started");
    let tasks_before = h.store.tasks(&Default::default()).unwrap().len();
    for (input, missing) in [
        (
            json!({"role":"worker", "objective":"check", "effort":"low"}),
            "model",
        ),
        (
            json!({"role":"worker", "objective":"check", "model":"claude-code/haiku"}),
            "effort",
        ),
    ] {
        let (reply, is_error) = h
            .runtime
            .engine_tool(coordinator, &bridge.token, "delegate".into(), input)
            .await
            .expect("bridge response");
        assert!(is_error && reply.contains(missing), "{reply}");
        assert!(children_of(h.store.as_ref(), coordinator).is_empty());
        assert_eq!(
            h.store.tasks(&Default::default()).unwrap().len(),
            tasks_before
        );
    }
    h.runtime.shutdown().await;
}

#[tokio::test]
async fn delegation_refuses_an_unregistered_engine_before_creating_a_child() {
    let mut h = harness(10).await;
    let coordinator = start_coordinator(&h, None);
    let bridge = h.ready.recv().await.expect("coordinator started");
    let tasks_before = h.store.tasks(&Default::default()).unwrap().len();
    let (reply, is_error) = h
        .runtime
        .engine_tool(
            coordinator,
            &bridge.token,
            "delegate".into(),
            json!({"effort":"low", "role": "worker", "objective": "read docs", "model": "codex/gpt-6-luna", "wait": false}),
        )
        .await
        .expect("bridge response");
    assert!(is_error, "unregistered engine accepted: {reply}");
    assert!(reply.contains("engine"), "{reply}");
    assert!(reply.contains("Locally runnable models: "), "{reply}");
    let alternatives = reply.split("Locally runnable models: ").nth(1).unwrap();
    assert!(
        alternatives.contains("claude-code/claude-haiku-5-5"),
        "{reply}"
    );
    assert!(!alternatives.contains("codex/"), "{reply}");
    assert!(children_of(h.store.as_ref(), coordinator).is_empty());
    assert_eq!(
        h.store.tasks(&Default::default()).unwrap().len(),
        tasks_before
    );
    let (invalid, is_error) = h.runtime.engine_tool(
        coordinator, &bridge.token, "delegate".into(),
        json!({"effort":"low", "role": "worker", "objective": "read docs", "model": "missing-provider/missing-model", "wait": false}),
    ).await.expect("unknown model refusal");
    assert!(is_error, "{invalid}");
    assert!(
        invalid.contains("missing-provider/missing-model"),
        "{invalid}"
    );
    assert!(
        invalid.contains("claude-code/claude-haiku-5-5"),
        "{invalid}"
    );
    assert_eq!(
        h.store.tasks(&Default::default()).unwrap().len(),
        tasks_before
    );
    assert!(children_of(h.store.as_ref(), coordinator).is_empty());
    let (catalog, is_error) = h
        .runtime
        .engine_tool(coordinator, &bridge.token, "list_models".into(), json!({}))
        .await
        .expect("model catalog");
    assert!(!is_error, "{catalog}");
    let catalog: serde_json::Value = serde_json::from_str(&catalog).expect("catalog JSON");
    assert_eq!(catalog["providers"]["codex"]["configured"], true);
    assert_eq!(catalog["providers"]["codex"]["available"], false);
    assert_eq!(catalog["providers"]["claude-code"]["available"], true);
    for entry in catalog["models"].as_array().expect("models") {
        assert!(entry.get("tier").is_some(), "tier key missing: {entry}");
    }
    let tier_of = |id: &str| {
        let entry = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .chain(catalog["configured_alternatives"].as_array().unwrap())
            .find(|e| e["model"]["model"] == id)
            .unwrap_or_else(|| panic!("{id} missing"));
        entry["tier"].clone()
    };
    assert_eq!(tier_of("claude-haiku-5-5"), "fast");
    assert_eq!(tier_of("claude-opus-5-5"), "strong");
    let guidance = catalog["guidance"]
        .as_str()
        .expect("guidance")
        .to_lowercase();
    for info in h.runtime.models().list() {
        for name in std::iter::once(&info.model.model).chain(&info.aliases) {
            assert!(
                !guidance.contains(&name.to_lowercase()),
                "guidance names model `{name}`: {guidance}"
            );
        }
    }
    let (reply, is_error) = h.runtime.engine_tool(
        coordinator, &bridge.token, "delegate".into(),
        json!({"model":"claude-code/haiku", "effort":"low", "role": "worker", "objective": "offline check", "network": false, "wait": false}),
    ).await.expect("offline refusal");
    assert!(is_error, "{reply}");
    assert!(reply.contains("offline dependency missing"), "{reply}");
    assert!(reply.contains("Locally runnable models: none"), "{reply}");
    assert!(children_of(h.store.as_ref(), coordinator).is_empty());
    assert_eq!(
        h.store.tasks(&Default::default()).unwrap().len(),
        tasks_before
    );
    let (child, _) = delegate(&h, &bridge, "ordinary read-only check").await;
    assert!(children_of(h.store.as_ref(), coordinator).contains(&child));
    h.runtime.shutdown().await;
}

/// When the requested provider is not configured but the same model id
/// is served by an available provider, the refusal names that concrete ref
/// instead of the full runnable list.
#[tokio::test]
async fn delegation_refusal_names_the_same_model_id_on_a_configured_provider() {
    let mut h = harness_with_providers(
        10,
        r#"
[providers.acme]
protocol = "openai-chat-completions"
base_url = "https://acme.example"
[providers.acme.auth]
api_key_env = ["ACME_TEST_KEY_UNSET"]
[[providers.acme.add_models]]
id = "shared-echo"

[[providers.claude-code.add_models]]
id = "shared-echo"
"#,
    )
    .await;
    let coordinator = start_coordinator(&h, None);
    let bridge = h.ready.recv().await.expect("coordinator started");
    let (reply, is_error) = h
        .runtime
        .engine_tool(
            coordinator,
            &bridge.token,
            "delegate".into(),
            json!({"effort":"low", "role": "worker", "objective": "read docs", "model": "acme/shared-echo", "wait": false}),
        )
        .await
        .expect("bridge response");
    assert!(is_error, "unconfigured provider accepted: {reply}");
    assert!(
        reply.contains("provider authentication is not configured"),
        "{reply}"
    );
    assert!(
        reply.contains("The same model is available as `claude-code/shared-echo`"),
        "{reply}"
    );
    assert!(!reply.contains("Locally runnable models: "), "{reply}");
    assert!(children_of(h.store.as_ref(), coordinator).is_empty());
    h.runtime.shutdown().await;
}

/// With two available providers serving the requested model id, the one
/// on the parent's own engine protocol is named, not the alphabetically first.
#[tokio::test]
async fn delegation_refusal_prefers_the_parents_own_engine_family() {
    let mut h = harness_with_providers(
        10,
        r#"
[providers.aaa-other]
protocol = "openai-chat-completions"
base_url = "https://aaa-other.example"
[[providers.aaa-other.add_models]]
id = "twin-model"

[providers.zzz-match]
protocol = "claude-code"
base_url = "https://zzz-match.example"
[[providers.zzz-match.add_models]]
id = "twin-model"

[providers.unavail-provider]
protocol = "openai-responses"
base_url = "https://unavail.example"
[providers.unavail-provider.auth]
api_key_env = ["UNAVAIL_TEST_KEY_UNSET"]
[[providers.unavail-provider.add_models]]
id = "twin-model"
"#,
    )
    .await;
    // The coordinator's own model is claude-code/haiku, so `zzz-match` (also
    // protocol `claude-code`) is the engine family it is already running.
    let coordinator = start_coordinator(&h, None);
    let bridge = h.ready.recv().await.expect("coordinator started");
    let (reply, is_error) = h
        .runtime
        .engine_tool(
            coordinator,
            &bridge.token,
            "delegate".into(),
            json!({"effort":"low", "role": "worker", "objective": "read docs", "model": "unavail-provider/twin-model", "wait": false}),
        )
        .await
        .expect("bridge response");
    assert!(is_error, "unconfigured provider accepted: {reply}");
    assert!(
        reply.contains("The same model is available as `zzz-match/twin-model`"),
        "{reply}"
    );
    assert!(!reply.contains("aaa-other"), "{reply}");
    assert!(children_of(h.store.as_ref(), coordinator).is_empty());
    h.runtime.shutdown().await;
}

/// When no other provider serves the requested model id, the refusal
/// keeps the plain reason-plus-runnable-list shape unchanged.
#[tokio::test]
async fn delegation_refusal_keeps_the_plain_list_with_no_same_id_match() {
    let mut h = harness_with_providers(
        10,
        r#"
[providers.solo-provider]
protocol = "openai-chat-completions"
base_url = "https://solo.example"
[providers.solo-provider.auth]
api_key_env = ["SOLO_TEST_KEY_UNSET"]
[[providers.solo-provider.add_models]]
id = "solo-id"
"#,
    )
    .await;
    let coordinator = start_coordinator(&h, None);
    let bridge = h.ready.recv().await.expect("coordinator started");
    let (reply, is_error) = h
        .runtime
        .engine_tool(
            coordinator,
            &bridge.token,
            "delegate".into(),
            json!({"effort":"low", "role": "worker", "objective": "read docs", "model": "solo-provider/solo-id", "wait": false}),
        )
        .await
        .expect("bridge response");
    assert!(is_error, "unconfigured provider accepted: {reply}");
    assert!(
        reply.contains("provider authentication is not configured"),
        "{reply}"
    );
    assert!(!reply.contains("The same model is available as"), "{reply}");
    assert!(reply.contains("Locally runnable models: "), "{reply}");
    let alternatives = reply.split("Locally runnable models: ").nth(1).unwrap();
    assert!(
        alternatives.contains("claude-code/claude-haiku-5-5"),
        "{reply}"
    );
    assert!(children_of(h.store.as_ref(), coordinator).is_empty());
    h.runtime.shutdown().await;
}

/// Nothing runnable under the requested network policy still gets the
/// existing "none" wording, not a spurious same-id hint.
#[tokio::test]
async fn delegation_refusal_reports_none_runnable_under_the_network_policy() {
    let mut h = harness(10).await;
    let coordinator = start_coordinator(&h, None);
    let bridge = h.ready.recv().await.expect("coordinator started");
    let (reply, is_error) = h
        .runtime
        .engine_tool(
            coordinator,
            &bridge.token,
            "delegate".into(),
            json!({"model":"claude-code/haiku", "effort":"low", "role": "worker", "objective": "offline check", "network": false, "wait": false}),
        )
        .await
        .expect("offline refusal");
    assert!(is_error, "{reply}");
    assert!(reply.contains("offline dependency missing"), "{reply}");
    assert!(
        reply.contains(
            "Locally runnable models: none under the requested network policy; use list_models to inspect provider readiness"
        ),
        "{reply}"
    );
    assert!(children_of(h.store.as_ref(), coordinator).is_empty());
    h.runtime.shutdown().await;
}
