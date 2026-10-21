#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! `add_models` entries in `providers.toml`, loaded from a real file: the
//! documented way to give a Claude Code model a 1M context window, capability
//! inheritance through `capabilities_from`, and the entries that are refused.

use std::path::PathBuf;

use aether_core::model::{ModelCost, ModelRef, ThinkingSupport};
use aether_model::config::PROVIDERS_CLAUDE_1M_EXAMPLE;
use aether_model::{ModelInfo, Models, Protocol};

const OPUS_1M: &str = "claude-opus-5-5[1m]";

/// A configuration directory holding only this `providers.toml`.
fn config_dir(providers_toml: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("config dir");
    std::fs::write(dir.path().join("providers.toml"), providers_toml).expect("providers.toml");
    dir
}

fn load(providers_toml: &str) -> Models {
    let dir = config_dir(providers_toml);
    Models::load(Some(dir.path().to_path_buf())).expect("providers.toml loads")
}

/// The message a `providers.toml` is refused with.
fn refusal(providers_toml: &str) -> String {
    let dir = config_dir(providers_toml);
    Models::load(Some(dir.path().to_path_buf()))
        .expect_err("providers.toml must be refused")
        .to_string()
}

fn claude_code_rows(models: &Models) -> Vec<ModelInfo> {
    models
        .list()
        .into_iter()
        .filter(|info| info.model.provider == "claude-code")
        .collect()
}

/// What `aether models` shows for one row: id, aliases, window, output limit.
fn row(info: &ModelInfo) -> (String, Vec<String>, u32, u32) {
    (
        info.model.model.clone(),
        info.aliases.clone(),
        info.capabilities.context_window,
        info.capabilities.max_output_tokens,
    )
}

#[test]
fn a_fresh_configuration_lists_the_claude_code_models_at_200k_only() {
    let dir = tempfile::tempdir().expect("config dir");
    let models = Models::load(Some(dir.path().to_path_buf())).expect("fresh configuration");
    let aliases = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
    let expected: Vec<(String, Vec<String>, u32, u32)> = vec![
        (
            "claude-opus-5-5".into(),
            aliases(&["opus", "opus-5-5", "opus-5.5"]),
            1_000_000,
            128_000,
        ),
        ("default".into(), Vec::new(), 1_000_000, 64_000),
        (
            "claude-fable-5-1".into(),
            aliases(&["fable", "fable-5-1"]),
            1_000_000,
            128_000,
        ),
        (
            "claude-sonnet-5-5".into(),
            aliases(&["sonnet", "sonnet-5-5", "sonnet-5.5"]),
            1_000_000,
            128_000,
        ),
        (
            "claude-haiku-5-5".into(),
            aliases(&["haiku", "haiku-5-5", "haiku-5.5"]),
            1_000_000,
            128_000,
        ),
    ];
    let listed: Vec<_> = claude_code_rows(&models).iter().map(row).collect();
    assert_eq!(listed, expected);
    assert!(
        models
            .resolve(&ModelRef::new("claude-code", OPUS_1M))
            .is_err(),
        "the 1M variant must not resolve until providers.toml adds it"
    );
}

#[test]
fn the_documented_edit_adds_claude_opus_with_a_1m_window() {
    let fresh_dir = tempfile::tempdir().expect("config dir");
    let fresh = Models::load(Some(fresh_dir.path().to_path_buf())).expect("fresh configuration");
    let models = load(PROVIDERS_CLAUDE_1M_EXAMPLE);

    let reference = ModelRef::parse(&format!("claude-code/{OPUS_1M}")).expect("model reference");
    let added = models
        .resolve(&reference)
        .expect("the added model resolves");
    assert_eq!(added.model.id, OPUS_1M);
    assert_eq!(added.model.name, "Claude Opus 5.5 (1M context)");
    assert_eq!(added.provider.protocol, Protocol::ClaudeCode);
    assert!(added.model.aliases.is_empty());

    let base = models
        .resolve(&ModelRef::new("claude-code", "opus"))
        .expect("the base model resolves");
    assert_eq!(base.model.id, "claude-opus-5-5");
    assert_eq!(base.model.capabilities.context_window, 1_000_000);

    // Everything but the window is the base model's.
    let mut expected = base.model.capabilities.clone();
    expected.context_window = 1_000_000;
    assert_eq!(added.model.capabilities, expected);
    assert_eq!(added.model.capabilities.max_output_tokens, 128_000);
    assert_eq!(added.model.capabilities.thinking, ThinkingSupport::Adaptive);
    assert!(added.model.capabilities.effort);
    assert!(added.model.capabilities.tools);
    assert!(added.model.capabilities.vision);
    assert_eq!(
        added.model.capabilities.cost,
        Some(ModelCost {
            input_per_mtok: 4.0,
            output_per_mtok: 20.0,
            cache_read_per_mtok: 0.20,
            cache_write_per_mtok: 5.0,
            cache_write_1h_per_mtok: Some(8.0),
            above_threshold: None,
        })
    );

    // Listed after the built-in rows, which are exactly a fresh install's.
    let mut rows = claude_code_rows(&models);
    let listed = rows.pop().expect("the added model is listed");
    assert_eq!(listed.model, reference);
    assert_eq!(listed.name, "Claude Opus 5.5 (1M context)");
    assert_eq!(listed.capabilities.context_window, 1_000_000);
    assert_eq!(rows, claude_code_rows(&fresh));
}

/// The snippet users copy from the documentation is the one tested above.
#[test]
fn the_documentation_shows_the_tested_snippet_verbatim() {
    let guide = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/engines.md");
    let text = std::fs::read_to_string(&guide).expect("docs/engines.md");
    let block = format!("```toml\n{PROVIDERS_CLAUDE_1M_EXAMPLE}```");
    assert!(
        text.contains(&block),
        "docs/engines.md must show PROVIDERS_CLAUDE_1M_EXAMPLE unchanged"
    );
}

#[test]
fn a_stated_capability_replaces_the_inherited_one_and_nothing_else() {
    let models = load(
        r#"
[[providers.claude-code.add_models]]
id = "opus-wide"
capabilities_from = "opus"
capabilities = { context_window = 500000, max_output_tokens = 64000, vision = false, extensions = {}, cost = { input_per_mtok = 8.0, output_per_mtok = 40.0 } }

[[providers.claude-code.add_models]]
id = "opus-wide-copy"
capabilities_from = "opus-wide"
"#,
    );
    let resolve = |id: &str| {
        models
            .resolve(&ModelRef::new("claude-code", id))
            .expect(id)
            .model
            .capabilities
    };
    let opus = resolve("claude-opus-5-5");
    let wide = resolve("opus-wide");
    assert_eq!(wide.context_window, 500_000);
    assert_eq!(wide.max_output_tokens, 64_000);
    // A stated table replaces the inherited one whole: no cache rates carry over.
    assert_eq!(
        wide.cost,
        Some(ModelCost {
            input_per_mtok: 8.0,
            output_per_mtok: 40.0,
            ..Default::default()
        })
    );
    assert_eq!(wide.thinking, opus.thinking);
    assert_eq!(wide.effort, opus.effort);
    assert!(opus.vision && !wide.vision);
    // An earlier entry of the same file is a base like any listed model.
    assert_eq!(resolve("opus-wide-copy"), wide);
}

#[test]
fn an_entry_without_capabilities_from_inherits_nothing() {
    let models = load(
        r#"
[[providers.claude-code.add_models]]
id = "bare"

[[providers.claude-code.add_models]]
id = "complete"
capabilities = { context_window = 300000, max_output_tokens = 32000 }
"#,
    );
    let resolve = |id: &str| {
        models
            .resolve(&ModelRef::new("claude-code", id))
            .expect(id)
            .model
            .capabilities
    };
    let mut bare = resolve("bare");
    assert_eq!(
        bare.compaction_tokens,
        Some(128_000),
        "clamped to its window"
    );
    bare.compaction_tokens = None;
    assert_eq!(bare, Default::default());
    let complete = resolve("complete");
    assert_eq!(complete.context_window, 300_000);
    assert_eq!(complete.max_output_tokens, 32_000);
    assert_eq!(complete.cost, None);
    assert!(!complete.effort);
}

#[test]
fn a_malformed_entry_is_refused_naming_the_file_and_the_entry() {
    let entry = |body: &str| format!("[[providers.claude-code.add_models]]\n{body}\n");
    for (body, expected) in [
        (
            "id = \"claude-opus-5-5[1m]\"\ncapabilities_from = \"claude-opus-5-5\"\n\
             capabilities = { context_window = 0 }",
            "provider `claude-code` model `claude-opus-5-5[1m]`: `context_window` must be \
             greater than 0",
        ),
        (
            "id = \"\"\nname = \"Nameless\"\ncapabilities_from = \"claude-opus-5-5\"",
            "provider `claude-code` lists a model with an empty `id` (name `Nameless`)",
        ),
        (
            "id = \"claude-opus-5-5[1m]\"\ncapabilities_from = \"claude-opus-9\"",
            "provider `claude-code` model `claude-opus-5-5[1m]`: `capabilities_from` names \
             `claude-opus-9`, which this provider does not list",
        ),
        (
            "id = \"claude-opus-5-5[1m]\"\ncapabilities_from = \"claude-opus-5-5\"\n\
             capabilities = { context_windw = 1000000 }",
            "provider `claude-code` model `claude-opus-5-5[1m]`: capabilities: unknown field \
             `context_windw`",
        ),
        (
            "id = \"claude-opus-5-5[1m]\"\ncapabilities = { context_window = 1000000 }",
            "provider `claude-code` model `claude-opus-5-5[1m]`: capabilities: missing field \
             `max_output_tokens`",
        ),
    ] {
        let message = refusal(&entry(body));
        assert!(message.starts_with("providers.toml: "), "{message}");
        assert!(message.contains(expected), "{message}");
    }
}
