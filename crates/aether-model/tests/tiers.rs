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
        ("claude-opus-5-5", ModelTier::Strong),
        ("claude-fable-5-1", ModelTier::Frontier),
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
        ("gpt-6-astra", ModelTier::Strong),
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
        "[models.tiers]\n\"claude-code/sonnet\" = \"frontier\"\n\"codex/gpt-6-luna\" = \"balanced\"\n",
        "",
    );
    assert_eq!(
        tier(&models, "claude-code", "claude-sonnet-5-5"),
        Some(ModelTier::Frontier)
    );
    assert_eq!(
        tier(&models, "claude-code", "sonnet"),
        Some(ModelTier::Frontier)
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
    assert!(error.contains("frontier"), "{error}");
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

/// Four tiered models anchor the bands; the rest are ranked by output price.
fn acme() -> String {
    let model = |id: &str, output: f64, tier: Option<&str>| {
        let tier = tier.map_or(String::new(), |t| format!("tier = \"{t}\"\n"));
        format!(
            "[[providers.acme.models]]\nid = \"{id}\"\n{tier}[providers.acme.models.capabilities]\ncontext_window = 1000\nmax_output_tokens = 100\ncost = {{ input_per_mtok = 1.0, output_per_mtok = {output}, cache_read_per_mtok = 0.0, cache_write_per_mtok = 0.0 }}\n\n"
        )
    };
    let mut toml = String::from(
        "[providers.acme]\nname = \"Acme\"\nprotocol = \"openai-chat-completions\"\nbase_url = \"http://localhost:1/v1\"\n\n",
    );
    for (id, output, tier) in [
        ("ref-frontier", 40.0, Some("frontier")),
        ("ref-strong", 16.0, Some("strong")),
        ("ref-balanced", 6.0, Some("balanced")),
        ("ref-fast", 0.4, Some("fast")),
        ("near-frontier", 30.0, None),
        ("near-strong", 20.0, None),
        ("near-balanced", 7.0, None),
        ("near-fast", 0.1, None),
    ] {
        toml.push_str(&model(id, output, tier));
    }
    toml.push_str(
        "[[providers.acme.models]]\nid = \"unpriced\"\n[providers.acme.models.capabilities]\ncontext_window = 1000\nmax_output_tokens = 100\n",
    );
    toml
}

#[test]
fn a_priced_model_without_a_tier_takes_the_nearest_tiered_models_band() {
    let models = load("", &acme());
    for (id, want) in [
        ("ref-strong", ModelTier::Strong),
        ("near-frontier", ModelTier::Frontier),
        ("near-strong", ModelTier::Strong),
        ("near-balanced", ModelTier::Balanced),
        ("near-fast", ModelTier::Fast),
    ] {
        assert_eq!(tier(&models, "acme", id), Some(want), "{id}");
    }
}

#[test]
fn an_unpriced_or_unreferenced_model_stays_unranked() {
    let models = load("", &acme());
    assert_eq!(tier(&models, "acme", "unpriced"), None);
    // No tiered model in groq to rank against.
    let models = load(
        "",
        "[[providers.groq.add_models]]\nid = \"priced\"\n[providers.groq.add_models.capabilities]\ncontext_window = 1000\nmax_output_tokens = 100\ncost = { input_per_mtok = 1.0, output_per_mtok = 2.0, cache_read_per_mtok = 0.0, cache_write_per_mtok = 0.0 }\n",
    );
    assert_eq!(tier(&models, "groq", "priced"), None);
}

#[test]
fn the_legacy_strongest_name_is_accepted_as_strong() {
    let models = load("[models.tiers]\n\"codex/gpt-6-luna\" = \"strongest\"\n", "");
    assert_eq!(tier(&models, "codex", "luna"), Some(ModelTier::Strong));
}

#[test]
fn the_model_list_applies_overrides_and_leaves_pass_through_models_unranked() {
    let models = load("[models.tiers]\n\"codex/luna\" = \"strongest\"\n", "");
    for info in models.list() {
        assert_eq!(info.tier, models.tier(&info.model), "{}", info.model);
        if info.model.provider == "codex" && info.model.model == "gpt-6-luna" {
            assert_eq!(info.tier, Some(ModelTier::Strong));
        }
        if ["groq", "openrouter", "xai", "google"].contains(&info.model.provider.as_str())
            || info.model.model == "default"
        {
            assert_eq!(info.tier, None, "{}", info.model);
        }
    }
}

#[test]
fn an_override_beats_the_price_fallback() {
    let models = load("[models.tiers]\n\"acme/near-strong\" = \"fast\"\n", &acme());
    assert_eq!(tier(&models, "acme", "near-strong"), Some(ModelTier::Fast));
}

#[test]
fn subscription_price_fallback_uses_the_api_equivalent() {
    let models = load(
        "",
        r#"
[[providers.anthropic.add_models]]
id = "new-model"
capabilities_from = "sonnet"
[providers.anthropic.add_models.capabilities.cost]
input_per_mtok = 2.0
output_per_mtok = 18.0
cache_read_per_mtok = 0.0
cache_write_per_mtok = 0.0

[[providers.claude-code.add_models]]
id = "new-model"
"#,
    );
    assert_eq!(
        tier(&models, "claude-code", "new-model"),
        Some(ModelTier::Strong)
    );
}
