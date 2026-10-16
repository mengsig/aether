use super::*;

#[tokio::test]
async fn refreshing_disabled_probes_returns_provider_status_without_spending() {
    let (daemon, _config) = daemon().await;
    let Reply::ProviderUsage(data) = dispatch(
        &daemon,
        Command::RefreshProviderUsage { provider: None },
        Actor::user(),
    )
    .await
    .unwrap() else {
        panic!("provider usage reply");
    };
    let providers = data["providers"].as_array().unwrap();
    assert!(!providers.is_empty());
    assert_eq!(
        providers.len(),
        daemon
            .models
            .registry()
            .providers()
            .iter()
            .filter(|p| p.enabled)
            .count()
    );
    for provider in providers {
        let engine_provider =
            matches!(provider["provider"].as_str(), Some("codex" | "claude-code"));
        assert_eq!(
            provider["status"],
            if engine_provider {
                "error"
            } else {
                "unsupported"
            }
        );
        if engine_provider {
            assert!(provider["error"].as_str().unwrap().contains("unavailable"));
        }
        assert_eq!(provider["headroom"]["stale"], true);
        assert_eq!(provider["headroom"]["windows"], serde_json::json!([]));
    }
    let Reply::ProviderUsage(filtered) = dispatch(
        &daemon,
        Command::RefreshProviderUsage {
            provider: Some("codex".into()),
        },
        Actor::user(),
    )
    .await
    .unwrap() else {
        panic!("provider usage reply");
    };
    assert_eq!(filtered["providers"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["providers"][0]["provider"], "codex");
    daemon.runtime.shutdown().await;
}
