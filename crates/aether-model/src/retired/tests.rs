use aether_core::model::ModelRef;

use crate::registry::{ProvidersFile, Registry, RegistryError};

use super::{RETIRED, successor};

/// The built-ins with the operator-configured `openai-codex` endpoint turned on.
fn every_provider_enabled() -> Registry {
    let mut registry = Registry::builtin();
    let enable: ProvidersFile = toml::from_str("[providers.openai-codex]\nenabled = true").unwrap();
    registry.apply_overrides(enable).unwrap();
    registry
}

#[test]
fn every_retired_id_and_alias_fails_with_a_resolvable_successor() {
    let registry = every_provider_enabled();
    for entry in RETIRED {
        for provider in entry.providers {
            let replacement = ModelRef::new(*provider, entry.successor);
            registry.resolve(&replacement).expect("successor resolves");
            for id in entry.ids {
                let model = ModelRef::new(*provider, *id);
                assert_eq!(successor(provider, id), Some(entry.successor));
                let error = registry.resolve(&model).expect_err("retired model fails");
                assert!(
                    matches!(&error, RegistryError::UnknownModel { provider: p, model: m, .. }
                        if p == provider && m == id)
                );
                assert_eq!(
                    error.to_string(),
                    format!("{model} was retired; use {replacement}")
                );
            }
        }
    }
}

#[test]
fn retirement_guidance_is_specific_to_the_builtin_route() {
    for (provider, id) in [
        ("custom", "gpt-6-sol"),
        ("openrouter", "gpt-5.6-luna"),
        ("codex", "terra"),
        ("openai", "gpt-5.6-terra"),
        ("anthropic", "gpt-6-sol"),
        ("codex", "claude-opus-5"),
        ("codex", "gpt-5.4"),
        ("claude-code", "claude-opus-4-6"),
        ("codex", "gpt-6.1-sol"),
        ("claude-code", "opus"),
    ] {
        assert_eq!(successor(provider, id), None, "{provider}/{id}");
    }
    let registry = Registry::builtin();
    let error = registry
        .resolve(&ModelRef::new("codex", "unknown-model"))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("unknown model `unknown-model` for provider `codex`; valid choices: "),
        "{error}"
    );
    assert!(matches!(
        registry.resolve(&ModelRef::new("custom", "gpt-6-sol")),
        Err(RegistryError::UnknownProvider(_))
    ));
}

#[test]
fn provider_overrides_resolve_retired_ids_and_aliases_before_guidance() {
    let mut registry = Registry::builtin();
    let overrides: ProvidersFile = toml::from_str(
        r#"
[[providers.codex.add_models]]
id = "gpt-5.6-luna"
aliases = ["spark"]
[[providers.anthropic.models]]
id = "claude-haiku-4-5"
aliases = ["haiku-4-5", "claude-haiku-4-5-20251001"]
[[providers.openai.add_models]]
id = "operator-model"
aliases = ["gpt-6-sol"]
"#,
    )
    .unwrap();
    registry.apply_overrides(overrides).unwrap();
    for (provider, id, canonical) in [
        ("codex", "gpt-5.6-luna", "gpt-5.6-luna"),
        ("codex", "spark", "gpt-5.6-luna"),
        ("anthropic", "claude-haiku-4-5", "claude-haiku-4-5"),
        ("anthropic", "haiku-4-5", "claude-haiku-4-5"),
        ("anthropic", "claude-haiku-4-5-20251001", "claude-haiku-4-5"),
        ("openai", "gpt-6-sol", "operator-model"),
    ] {
        let resolved = registry.resolve(&ModelRef::new(provider, id)).unwrap();
        assert_eq!(resolved.model.id, canonical);
    }
}

#[test]
fn open_model_lists_reject_retired_ids_but_keep_unknown_models_and_disabled_errors() {
    let mut registry = Registry::builtin();
    let overrides: ProvidersFile = toml::from_str(
        r#"
[providers.codex.flags]
open_model_list = true
[providers.anthropic]
enabled = false
"#,
    )
    .unwrap();
    registry.apply_overrides(overrides).unwrap();
    assert!(matches!(
        registry
            .resolve(&ModelRef::new("codex", "gpt-6-sol"))
            .unwrap_err(),
        RegistryError::UnknownModel { .. }
    ));
    assert_eq!(
        registry
            .resolve(&ModelRef::new("codex", "operator-model"))
            .unwrap()
            .model
            .id,
        "operator-model"
    );
    let overrides: ProvidersFile = toml::from_str("[providers.openai]\nmodels = []").unwrap();
    registry.apply_overrides(overrides).unwrap();
    assert_eq!(
        registry
            .resolve(&ModelRef::new("openai", "gpt-5.4"))
            .unwrap_err()
            .to_string(),
        "openai/gpt-5.4 was retired; use openai/gpt-6.1-sol"
    );
    assert!(matches!(
        registry.resolve(&ModelRef::new("anthropic", "claude-opus-5")),
        Err(RegistryError::DisabledProvider(_))
    ));
}

const CODEX_REMOVED: &[&str] = &[
    "gpt-6-sol",
    "gpt-5.6-sol",
    "gpt-5.6",
    "gpt-5.6-luna",
    "gpt-5.5",
    "gpt-5.3-codex-spark",
    "spark",
];
const OPENAI_API_REMOVED: &[&str] = &[
    "gpt-6-sol",
    "gpt-5.6-sol",
    "gpt-5.6",
    "gpt-5.5",
    "gpt-5.4",
    "gpt-5.6-luna",
    "gpt-5.4-mini",
];

/// Every id and alias the 93d18647 catalogue shipped and this one dropped,
/// per first-party provider.
const REMOVED: &[(&str, &[&str])] = &[
    (
        "anthropic",
        &[
            "claude-opus-5",
            "opus-5",
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-sonnet-5",
            "sonnet-5",
            "claude-sonnet-4-6",
            "claude-haiku-4-5",
            "haiku-4-5",
            "claude-haiku-4-5-20251001",
            "claude-fable-5",
        ],
    ),
    (
        "claude-code",
        &[
            "claude-opus-5",
            "opus-5",
            "claude-sonnet-5",
            "sonnet-5",
            "claude-haiku-4-5",
            "haiku-4-5",
            "claude-haiku-4-5-20251001",
        ],
    ),
    ("codex", CODEX_REMOVED),
    ("openai-codex", CODEX_REMOVED),
    ("openai", OPENAI_API_REMOVED),
    ("openai-responses", OPENAI_API_REMOVED),
];

/// Open model lists (`openai-codex`) would otherwise pass a removed name
/// through as an unknown model.
#[test]
fn every_removed_catalogue_name_fails_with_guidance_on_an_enabled_provider() {
    let registry = every_provider_enabled();
    for (provider, removed) in REMOVED {
        for id in *removed {
            let model = ModelRef::new(*provider, *id);
            let error = registry.resolve(&model).expect_err("removed model fails");
            assert!(error.is_retirement(), "{model}: {error}");
            let successor = ModelRef::new(*provider, successor(provider, id).unwrap());
            registry.resolve(&successor).expect("successor resolves");
            assert_eq!(
                error.to_string(),
                format!("{model} was retired; use {successor}")
            );
        }
    }
}

#[test]
fn a_disabled_provider_still_refuses_removed_names_as_disabled() {
    let registry = Registry::builtin();
    for id in CODEX_REMOVED {
        let error = registry
            .resolve(&ModelRef::new("openai-codex", *id))
            .unwrap_err();
        assert_eq!(error.to_string(), "provider `openai-codex` is disabled");
    }
}
