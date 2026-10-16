use super::*;
use crate::registry::ToolRegistry;

#[tokio::test]
async fn refresh_tool_is_registered_and_preserves_optional_provider() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    let registry = ToolRegistry::with_builtins();
    let tool = registry.get("refresh_provider_usage").unwrap();
    let descriptor = tool.descriptor();
    assert_eq!(descriptor.side_effect, SideEffectClass::External);
    assert!(descriptor.needs_network);
    assert_eq!(descriptor.spec.input_schema["additionalProperties"], false);
    assert_eq!(
        descriptor.spec.input_schema["properties"]["provider"]["type"],
        "string"
    );
    assert_eq!(
        tool.resources(&json!({}), &ctx),
        vec![ResourceKey::network()]
    );
    for (input, provider) in [
        (json!({}), None),
        (json!({"provider":null}), None),
        (json!({"provider":"codex"}), Some("codex".into())),
        (
            json!({"provider":"custom-engine"}),
            Some("custom-engine".into()),
        ),
    ] {
        assert_eq!(
            act(tool.as_ref(), &ctx, input).await.unwrap(),
            HarnessAction::RefreshProviderUsage { provider }
        );
    }
    assert_eq!(
        act(&ProviderUsage, &ctx, json!({})).await.unwrap(),
        HarnessAction::ProviderUsage
    );
    assert!(!ProviderUsage.descriptor().needs_network);
}

#[tokio::test]
async fn refresh_tool_rejects_invalid_provider_and_unknown_input() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ctx_in(dir.path());
    for input in [
        json!({"provider":""}),
        json!({"provider":" \t\n"}),
        json!({"provider":42}),
        json!({"provider":false}),
        json!({"provider":[]}),
        json!({"provider":{}}),
        json!({"force":true}),
        json!({"provider":"codex", "unknown":true}),
        json!([]),
        json!(null),
    ] {
        let error = act(&RefreshProviderUsage, &ctx, input).await.unwrap_err();
        assert!(matches!(error, ToolError::InvalidInput(_)));
        assert!(!error.to_string().is_empty());
    }
}
