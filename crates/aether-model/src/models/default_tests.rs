//! Exercise CLI authentication without using the operator's CLIs or credentials.

use super::*;
use crate::auth::{AuthMethods, CliAuth, MemoryCredentialStore};
use crate::registry::{ProviderOverride, ProvidersFile};

fn configured(
    claude: bool,
    codex: bool,
    explicit: Option<&str>,
    disabled: Option<&str>,
) -> Result<Models, ModelsError> {
    let mut registry = Registry::builtin();
    let providers = [("claude-code", claude), ("codex", codex)]
        .into_iter()
        .map(|(id, authenticated)| {
            let response = if authenticated {
                "authenticated"
            } else {
                "logged-out"
            };
            let cli = CliAuth {
                program: "/bin/sh".into(),
                status: vec!["-c".into(), format!("printf '%s' '{response}'")],
                login: vec![],
                logout: vec![],
                logged_in_markers: vec!["authenticated".into()],
                label: "isolated test CLI".into(),
            };
            (
                id.into(),
                ProviderOverride {
                    auth: Some(AuthMethods::cli(cli)),
                    enabled: Some(disabled != Some(id)),
                    ..Default::default()
                },
            )
        })
        .collect();
    registry.apply_overrides(ProvidersFile { providers })?;
    let mut config = AetherConfig::default();
    config.model.default = explicit.map(str::to_owned);
    Ok(Models::new(
        registry,
        config,
        Arc::new(MemoryCredentialStore::default()),
        PathBuf::from("/nonexistent"),
    )?
    .with_env(|key| {
        matches!(key, "ANTHROPIC_API_KEY" | "OPENAI_API_KEY").then(|| "test-only-key".into())
    }))
}

#[tokio::test]
async fn authenticated_native_harnesses_precede_api_credentials() {
    for (claude, codex, expected) in [
        (true, true, "claude-code"),
        (true, false, "claude-code"),
        (false, true, "codex"),
    ] {
        assert_eq!(
            configured(claude, codex, None, None)
                .unwrap()
                .default_model()
                .await,
            Some(ModelRef::new(expected, "default",))
        );
    }
}

#[tokio::test]
async fn explicit_claude_choices_override_the_new_preference() {
    for model in ["default", "fable", "sonnet"] {
        assert_eq!(
            configured(true, true, Some(&format!("claude-code/{model}")), None)
                .unwrap()
                .default_model()
                .await,
            Some(ModelRef::new("claude-code", model))
        );
    }
}

#[tokio::test]
async fn explicit_codex_defaults_are_not_replaced_by_astra() {
    for model in ["default", "sol", "luna", "gpt-6.1-sol"] {
        assert_eq!(
            configured(false, true, Some(&format!("codex/{model}")), None)
                .unwrap()
                .default_model()
                .await,
            Some(ModelRef::new("codex", model))
        );
    }
}

#[tokio::test]
async fn logged_out_native_harnesses_fall_back_to_api_provider() {
    let models = configured(false, false, None, None).unwrap();
    let selected = models.default_model().await.unwrap();
    assert_eq!(selected.provider, "anthropic");
    assert_eq!(selected.model, "claude-opus-5-5");
    assert!(
        !models
            .resolve(&selected)
            .unwrap()
            .provider
            .protocol
            .is_engine()
    );
}

#[tokio::test]
async fn explicit_api_default_is_never_replaced_by_an_authenticated_cli() {
    assert_eq!(
        configured(true, true, Some("openai/gpt-6-astra"), None)
            .unwrap()
            .default_model()
            .await,
        Some(ModelRef::new("openai", "gpt-6-astra"))
    );
}

#[tokio::test]
async fn disabled_native_provider_is_not_automatically_selected() {
    assert_eq!(
        configured(true, true, None, Some("claude-code"))
            .unwrap()
            .default_model()
            .await,
        Some(ModelRef::new("codex", "default"))
    );
}

#[tokio::test]
async fn invalid_explicit_default_falls_back_to_authenticated_native() {
    assert_eq!(
        configured(false, true, Some("nonexistent/model"), None)
            .unwrap()
            .default_model()
            .await,
        Some(ModelRef::new("codex", "default"))
    );
}

#[tokio::test]
async fn retired_explicit_default_is_kept_for_refusal_not_replaced() {
    for (text, successor) in [
        ("codex/gpt-5.6-terra", "codex/gpt-6-luna"),
        ("claude-code/claude-opus-5", "claude-code/claude-opus-5-5"),
        ("anthropic/claude-sonnet-4-6", "anthropic/claude-sonnet-5-5"),
    ] {
        let models = configured(true, true, Some(text), None).unwrap();
        let selected = models.default_model().await.expect("a default");
        assert_eq!(selected.to_string(), text);
        assert_eq!(
            models.resolve(&selected).unwrap_err().to_string(),
            format!("{text} was retired; use {successor}")
        );
        assert_eq!(
            models.retired_settings(),
            [format!(
                "[model] default: {text} was retired; use {successor}"
            )]
        );
    }
}

#[tokio::test]
async fn an_override_defining_a_retired_default_still_selects_it() {
    let mut models = configured(true, true, Some("codex/gpt-5.6-terra"), None).unwrap();
    models
        .registry
        .apply_overrides(
            toml::from_str("[[providers.codex.add_models]]\nid = \"gpt-5.6-terra\"").unwrap(),
        )
        .unwrap();
    let selected = models.default_model().await.unwrap();
    assert_eq!(selected, ModelRef::new("codex", "gpt-5.6-terra"));
    models.resolve(&selected).expect("the override resolves");
    assert!(models.retired_settings().is_empty());
}

#[test]
fn a_retired_compactor_is_reported_with_its_successor() {
    let mut models = configured(false, false, None, None).unwrap();
    models.config.model.compactor = Some("codex/terra".into());
    assert_eq!(
        models.retired_settings(),
        ["[model] compactor: codex/terra was retired; use codex/gpt-6-luna"]
    );
    models.config.model.compactor = Some("codex/gpt-6-luna".into());
    assert!(models.retired_settings().is_empty());
}
