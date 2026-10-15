#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Model tiers: the catalog's declared bands, operator overrides and the
//! price fallback, as `Models::tier` reports them.

use aether_core::model::ModelRef;
use aether_model::{ModelTier, Models};

fn load(config_toml: &str, providers_toml: &str) -> Models {
    let dir = tempfile::tempdir().expect("config dir");
    std::fs::write(dir.path().join("config.toml"), config_toml).expect("config.toml");
    std::fs::write(dir.path().join("providers.toml"), providers_toml).expect("providers.toml");
    Models::load(Some(dir.path().to_path_buf())).expect("configuration loads")
}

fn tier(models: &Models, provider: &str, model: &str) -> Option<ModelTier> {
    models.tier(&ModelRef::new(provider, model))
}

#[test]
fn every_builtin_model_has_its_declared_tier() {
    let models = load("", "");
    let expected = [
        ("claude-opus-5-5", ModelTier::Strongest),
        ("claude-fable-5-1", ModelTier::Strongest),
        ("claude-sonnet-5-5", ModelTier::Balanced),
        ("claude-haiku-5-5", ModelTier::Fast),
    ];
    for provider in ["anthropic", "claude-code"] {
        for (model, want) in expected {
            assert_eq!(
                tier(&models, provider, model),
                Some(want),
                "{provider}/{model}"
            );
        }
    }
    let expected = [
        ("gpt-6-astra", ModelTier::Strongest),
        ("gpt-6.1-sol", ModelTier::Balanced),
        ("gpt-6-luna", ModelTier::Fast),
    ];
    for provider in ["codex", "openai", "openai-responses"] {
        for (model, want) in expected {
            assert_eq!(
                tier(&models, provider, model),
                Some(want),
                "{provider}/{model}"
            );
        }
    }
    // Aliases rank like the model they name; pass-through and `default` stay unranked.
    assert_eq!(tier(&models, "codex", "sol"), Some(ModelTier::Balanced));
    assert_eq!(tier(&models, "claude-code", "default"), None);
    assert_eq!(tier(&models, "codex", "default"), None);
    assert_eq!(tier(&models, "groq", "llama-3.3-70b-versatile"), None);
}

#[test]
fn a_config_override_beats_the_catalog_tier() {
    let models = load(
        "[models.tiers]\n\"claude-code/sonnet\" = \"strongest\"\n\"codex/gpt-6-luna\" = \"balanced\"\n",
        "",
    );
    assert_eq!(
        tier(&models, "claude-code", "claude-sonnet-5-5"),
        Some(ModelTier::Strongest)
    );
    assert_eq!(
        tier(&models, "claude-code", "sonnet"),
        Some(ModelTier::Strongest)
    );
    assert_eq!(tier(&models, "codex", "luna"), Some(ModelTier::Balanced));
    assert_eq!(
        tier(&models, "anthropic", "claude-sonnet-5-5"),
        Some(ModelTier::Balanced)
    );
}

#[test]
fn an_override_ranks_an_unranked_model() {
    let models = load(
        "[models.tiers]\n\"groq/openai/gpt-oss-120b\" = \"fast\"\n",
        "",
    );
    assert_eq!(
        tier(&models, "groq", "openai/gpt-oss-120b"),
        Some(ModelTier::Fast)
    );
}

#[test]
fn an_unknown_tier_name_is_a_config_error_naming_the_choices() {
    let dir = tempfile::tempdir().expect("config dir");
    std::fs::write(
        dir.path().join("config.toml"),
        "[models.tiers]\n\"codex/gpt-6-luna\" = \"cheap\"\n",
    )
    .expect("config.toml");
    let error = Models::load(Some(dir.path().to_path_buf()))
        .expect_err("must be refused")
        .to_string();
    assert!(error.contains("config.toml"), "{error}");
    assert!(error.contains("cheap"), "{error}");
    assert!(error.contains("strongest"), "{error}");
}

#[test]
fn an_override_key_must_be_provider_slash_model() {
    let dir = tempfile::tempdir().expect("config dir");
    std::fs::write(
        dir.path().join("config.toml"),
        "[models.tiers]\nopus = \"fast\"\n",
    )
    .expect("config.toml");
    let error = Models::load(Some(dir.path().to_path_buf()))
        .expect_err("must be refused")
        .to_string();
    assert!(error.contains("models.tiers key `opus`"), "{error}");
}

/// A provider with one declared tier and unlisted-tier priced siblings: the
/// siblings rank by output price against the dearest tiered model.
const LOCAL: &str = r#"
[providers.acme]
name = "Acme"
protocol = "openai-chat-completions"
base_url = "http://localhost:1/v1"

[[providers.acme.models]]
id = "acme-top"
tier = "strongest"
[providers.acme.models.capabilities]
context_window = 1000
max_output_tokens = 100
cost = { input_per_mtok = 5.0, output_per_mtok = 40.0, cache_read_per_mtok = 0.0, cache_write_per_mtok = 0.0 }

[[providers.acme.models]]
id = "acme-mid"
[providers.acme.models.capabilities]
context_window = 1000
max_output_tokens = 100
cost = { input_per_mtok = 1.0, output_per_mtok = 8.0, cache_read_per_mtok = 0.0, cache_write_per_mtok = 0.0 }

[[providers.acme.models]]
id = "acme-near-top"
[providers.acme.models.capabilities]
context_window = 1000
max_output_tokens = 100
cost = { input_per_mtok = 4.0, output_per_mtok = 20.0, cache_read_per_mtok = 0.0, cache_write_per_mtok = 0.0 }

[[providers.acme.models]]
id = "acme-small"
[providers.acme.models.capabilities]
context_window = 1000
max_output_tokens = 100
cost = { input_per_mtok = 0.1, output_per_mtok = 0.4, cache_read_per_mtok = 0.0, cache_write_per_mtok = 0.0 }

[[providers.acme.models]]
id = "acme-unpriced"
[providers.acme.models.capabilities]
context_window = 1000
max_output_tokens = 100
"#;

#[test]
fn a_priced_model_without_a_tier_ranks_against_its_providers_dearest_tiered_model() {
    let models = load("", LOCAL);
    assert_eq!(
        tier(&models, "acme", "acme-top"),
        Some(ModelTier::Strongest)
    );
    assert_eq!(
        tier(&models, "acme", "acme-near-top"),
        Some(ModelTier::Strongest)
    );
    assert_eq!(tier(&models, "acme", "acme-mid"), Some(ModelTier::Balanced));
    assert_eq!(tier(&models, "acme", "acme-small"), Some(ModelTier::Fast));
}

#[test]
fn an_unpriced_or_unreferenced_model_stays_unranked() {
    let models = load("", LOCAL);
    assert_eq!(tier(&models, "acme", "acme-unpriced"), None);
    // No tiered model in groq to rank against.
    let models = load(
        "",
        "[[providers.groq.add_models]]\nid = \"priced\"\n[providers.groq.add_models.capabilities]\ncontext_window = 1000
max_output_tokens = 100
cost = { input_per_mtok = 1.0, output_per_mtok = 2.0, cache_read_per_mtok = 0.0, cache_write_per_mtok = 0.0 }\n",
    );
    assert_eq!(tier(&models, "groq", "priced"), None);
}
