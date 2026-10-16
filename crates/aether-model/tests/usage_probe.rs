use aether_model::{ProviderDef, Registry, UsageProbe, registry::ProvidersFile};

#[test]
fn probe_turns_default_to_disabled_and_preserve_legacy_provider_definitions() {
    let registry = Registry::builtin();
    assert!(!registry.providers().is_empty());
    assert!(
        registry
            .providers()
            .iter()
            .all(|p| p.usage_probe == UsageProbe::Disabled)
    );
    let mut legacy = serde_json::to_value(registry.provider("codex").unwrap()).unwrap();
    legacy.as_object_mut().unwrap().remove("usage_probe");
    let decoded: ProviderDef = serde_json::from_value(legacy).unwrap();
    assert_eq!(decoded.usage_probe, UsageProbe::Disabled);
    assert_eq!(decoded.id, "codex");
}

#[test]
fn probe_overrides_require_explicit_opt_in_and_can_be_disabled_again() {
    let mut registry = Registry::builtin();
    for (settings, expected) in [
        ("usage_probe = 'turn'", UsageProbe::Turn),
        ("name = 'Custom Codex'", UsageProbe::Turn),
        ("usage_probe = 'disabled'", UsageProbe::Disabled),
    ] {
        let overrides: ProvidersFile =
            toml::from_str(&format!("[providers.codex]\n{settings}")).unwrap();
        registry.apply_overrides(overrides).unwrap();
        assert_eq!(registry.provider("codex").unwrap().usage_probe, expected);
        assert_eq!(
            registry.provider("claude-code").unwrap().usage_probe,
            UsageProbe::Disabled
        );
    }
    assert_eq!(registry.provider("codex").unwrap().name, "Custom Codex");
}

#[test]
fn custom_providers_merge_probe_policy_without_changing_enabled_state() {
    let mut registry = Registry::builtin();
    let overrides: ProvidersFile = toml::from_str(
        r#"
        [providers.custom-engine]
        protocol = "codex"
        base_url = ""
        usage_probe = "turn"
        [providers.custom-api]
        protocol = "openai-chat-completions"
        base_url = "https://example.com/v1"
        [providers.disabled-engine]
        protocol = "codex"
        base_url = ""
        usage_probe = "turn"
        enabled = false
        "#,
    )
    .unwrap();
    registry.apply_overrides(overrides).unwrap();
    assert_eq!(
        registry.provider("custom-engine").unwrap().usage_probe,
        UsageProbe::Turn
    );
    assert_eq!(
        registry.provider("custom-api").unwrap().usage_probe,
        UsageProbe::Disabled
    );
    let disabled = registry.provider("disabled-engine").unwrap();
    assert_eq!(disabled.usage_probe, UsageProbe::Turn);
    assert!(!disabled.enabled);
}

#[test]
fn probe_policy_rejects_unknown_values_and_wrong_types() {
    for invalid in ["'enabled'", "'auto'", "true", "1", "{}"] {
        let error =
            toml::from_str::<ProvidersFile>(&format!("[providers.codex]\nusage_probe = {invalid}"))
                .unwrap_err();
        assert!(error.to_string().contains("usage_probe"), "{error}");
    }
    assert_eq!(
        serde_json::to_value(UsageProbe::Disabled).unwrap(),
        "disabled"
    );
    assert_eq!(serde_json::to_value(UsageProbe::Turn).unwrap(), "turn");
}
